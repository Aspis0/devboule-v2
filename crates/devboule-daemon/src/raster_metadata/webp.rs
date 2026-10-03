//! The WebP walk: a whitelist over the RIFF chunk list, copied verbatim.
//!
//! A WebP is `RIFF`, a little-endian size, `WEBP` and then chunks, each a
//! four-letter type, a little-endian size, the payload and one pad byte when
//! the size is odd. Nothing here decodes a bitstream. Every chunk is located by
//! its declared size checked against what is left of the input, so each step
//! advances at least eight bytes, the walk is bounded by the input length, and
//! there is no decompression bomb to build.
//!
//! What survives is what draws the picture: the bitstream chunks, the extended
//! header, alpha, the animation header and its frames, and the ICC profile
//! (colour fidelity, as on the JPEG side). `EXIF`, `XMP ` and any chunk this
//! pass does not name leave. An animation frame is walked too: its payload
//! carries chunks of its own, and an unnamed one in there is as much a place to
//! hide a blob as one at the top.

use super::{add_rule, RasterMetadataRule, RasterStripError, StripResult};

const RIFF_HEADER_LEN: usize = 12;
const CHUNK_HEADER_LEN: usize = 8;
/// An animation frame's fixed header (position, size, duration, flags) before
/// the chunks of its own.
const FRAME_HEADER_LEN: usize = 16;
const EXTENDED_HEADER_LEN: usize = 10;

/// In the first byte of the extended header's flags.
const FLAG_XMP: u8 = 0x04;
const FLAG_EXIF: u8 = 0x08;

const TOP_LEVEL_CHUNKS: [&[u8]; 7] = [
    b"VP8 ", b"VP8L", b"VP8X", b"ALPH", b"ANIM", b"ANMF", b"ICCP",
];
const FRAME_CHUNKS: [&[u8]; 3] = [b"VP8 ", b"VP8L", b"ALPH"];

pub(super) fn has_webp_header(bytes: &[u8]) -> bool {
    bytes.len() >= RIFF_HEADER_LEN && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

pub(super) fn strip_webp_chunks(
    bytes: &[u8],
    removed: &mut Vec<RasterMetadataRule>,
) -> StripResult {
    if !has_webp_header(bytes) {
        return Err(RasterStripError::new(
            "it does not begin with RIFF, a size and WEBP".to_string(),
        ));
    }
    let declared = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    // u64 so the sum cannot wrap whatever the declared size is.
    let riff_end = 8 + u64::from(declared);
    if riff_end > bytes.len() as u64 {
        return Err(RasterStripError::new(format!(
            "the RIFF header declares {declared} bytes and runs past the end of the file"
        )));
    }
    let riff_end = riff_end as usize;
    if riff_end < RIFF_HEADER_LEN {
        return Err(RasterStripError::new(
            "the RIFF header declares fewer bytes than the WEBP signature".to_string(),
        ));
    }

    let top = copy_top_level_chunks(&bytes[RIFF_HEADER_LEN..riff_end], removed)?;
    if !top.saw_image {
        return Err(RasterStripError::new(
            "it carries no image data".to_string(),
        ));
    }
    if riff_end != bytes.len() {
        add_rule(removed, RasterMetadataRule::TrailingData);
    }
    let Ok(declared) = u32::try_from(top.chunks.len() + 4) else {
        return Err(RasterStripError::new(
            "the chunks do not fit a RIFF size".to_string(),
        ));
    };
    let mut out = Vec::with_capacity(RIFF_HEADER_LEN + top.chunks.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&declared.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&top.chunks);
    Ok(out)
}

/// One chunk read off a region: its type, its payload, and every byte it
/// occupies, pad byte included.
struct Chunk<'a> {
    fourcc: &'a [u8],
    payload: &'a [u8],
    whole: &'a [u8],
}

impl Chunk<'_> {
    /// Header and payload copied verbatim, then the pad byte the format fixes
    /// at zero: the input's own pad byte is a place to hide one byte, so it is
    /// never copied.
    fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.whole[..CHUNK_HEADER_LEN + self.payload.len()]);
        if self.payload.len() % 2 == 1 {
            out.push(0);
        }
    }
}

/// The chunk at `index` in `region`. `base` is where `region` begins in the
/// file, so a refusal names a file offset.
fn read_chunk(region: &[u8], index: usize, base: usize) -> Result<Chunk<'_>, RasterStripError> {
    let at = base + index;
    if index + CHUNK_HEADER_LEN > region.len() {
        return Err(RasterStripError::new(format!(
            "the chunk header at byte {at} is truncated"
        )));
    }
    let fourcc = &region[index..index + 4];
    let size = u32::from_le_bytes([
        region[index + 4],
        region[index + 5],
        region[index + 6],
        region[index + 7],
    ]);
    // u64 so the arithmetic cannot wrap whatever the declared size is; the
    // comparison against the real length is what makes it exact.
    let end = index as u64 + CHUNK_HEADER_LEN as u64 + u64::from(size) + u64::from(size & 1);
    if end > region.len() as u64 {
        return Err(RasterStripError::new(format!(
            "the {} chunk at byte {at} declares {size} data bytes and runs past the end of its container",
            String::from_utf8_lossy(fourcc)
        )));
    }
    let end = end as usize;
    Ok(Chunk {
        fourcc,
        payload: &region[index + CHUNK_HEADER_LEN..index + CHUNK_HEADER_LEN + size as usize],
        whole: &region[index..end],
    })
}

/// The extended-header flag that announces a metadata chunk, or none.
fn metadata_flag(fourcc: &[u8]) -> u8 {
    match fourcc {
        b"EXIF" => FLAG_EXIF,
        b"XMP " => FLAG_XMP,
        _ => 0,
    }
}

fn drop_rule(fourcc: &[u8]) -> RasterMetadataRule {
    match fourcc {
        b"EXIF" => RasterMetadataRule::Exif,
        b"XMP " => RasterMetadataRule::Xmp,
        _ => RasterMetadataRule::UnknownChunk,
    }
}

struct TopLevel {
    chunks: Vec<u8>,
    saw_image: bool,
}

/// The top-level chunks that survive, with the extended header's flags cleared
/// for the metadata chunks that did not, whether they sat at the top or inside
/// a frame.
fn copy_top_level_chunks(
    region: &[u8],
    removed: &mut Vec<RasterMetadataRule>,
) -> Result<TopLevel, RasterStripError> {
    let mut chunks = Vec::with_capacity(region.len());
    let mut index = 0;
    let mut saw_image = false;
    let mut flags_at = None;
    let mut dropped_flags = 0u8;

    while index < region.len() {
        let chunk = read_chunk(region, index, RIFF_HEADER_LEN)?;
        let payload_base = RIFF_HEADER_LEN + index + CHUNK_HEADER_LEN;
        index += chunk.whole.len();
        if !TOP_LEVEL_CHUNKS.contains(&chunk.fourcc) {
            add_rule(removed, drop_rule(chunk.fourcc));
            dropped_flags |= metadata_flag(chunk.fourcc);
            continue;
        }
        match chunk.fourcc {
            b"VP8X" => {
                if chunk.payload.len() != EXTENDED_HEADER_LEN || flags_at.is_some() {
                    return Err(RasterStripError::new(
                        "its VP8X chunk is not the single ten-byte header the format defines"
                            .to_string(),
                    ));
                }
                flags_at = Some(chunks.len() + CHUNK_HEADER_LEN);
                chunk.write_to(&mut chunks);
            }
            b"ANMF" => {
                saw_image = true;
                if chunk.payload.len() < FRAME_HEADER_LEN {
                    return Err(RasterStripError::new(
                        "an ANMF chunk is shorter than its frame header".to_string(),
                    ));
                }
                dropped_flags |= push_frame(&mut chunks, chunk.payload, payload_base, removed)?;
            }
            fourcc => {
                saw_image |= fourcc == b"VP8 " || fourcc == b"VP8L";
                chunk.write_to(&mut chunks);
            }
        }
    }

    if let Some(at) = flags_at {
        chunks[at] &= !dropped_flags;
    }
    Ok(TopLevel { chunks, saw_image })
}

/// An `ANMF` chunk rebuilt around its own surviving chunks, with the size that
/// goes with them. Answers the extended-header flags of the metadata chunks it
/// dropped.
fn push_frame(
    chunks: &mut Vec<u8>,
    payload: &[u8],
    base: usize,
    removed: &mut Vec<RasterMetadataRule>,
) -> Result<u8, RasterStripError> {
    let mut body = payload[..FRAME_HEADER_LEN].to_vec();
    let region = &payload[FRAME_HEADER_LEN..];
    let mut index = 0;
    let mut dropped_flags = 0u8;
    while index < region.len() {
        let chunk = read_chunk(region, index, base + FRAME_HEADER_LEN)?;
        index += chunk.whole.len();
        if FRAME_CHUNKS.contains(&chunk.fourcc) {
            chunk.write_to(&mut body);
        } else {
            add_rule(removed, drop_rule(chunk.fourcc));
            dropped_flags |= metadata_flag(chunk.fourcc);
        }
    }
    chunks.extend_from_slice(b"ANMF");
    // The frame header is sixteen bytes and every chunk copied above carries
    // its own pad, so `body` is even and the ANMF needs no pad of its own.
    chunks.extend_from_slice(&(body.len() as u32).to_le_bytes());
    chunks.extend_from_slice(&body);
    Ok(dropped_flags)
}
