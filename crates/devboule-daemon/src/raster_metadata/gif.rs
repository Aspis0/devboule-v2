//! The GIF walk: a whitelist over the block stream, copied verbatim.
//!
//! A GIF is a header, a screen descriptor, an optional colour table and then
//! blocks until a trailer: image descriptors, and extensions that are each a
//! label followed by a chain of length-prefixed sub-blocks ended by a zero
//! length. Nothing here decodes the LZW data, so the walk allocates no more
//! than the input and every step advances at least one byte — the loop is
//! bounded by the input length and there is no decompression bomb to build.
//!
//! What survives is what draws the picture: image descriptors with their
//! colour tables and data, graphic control extensions (frame timing and
//! transparency), plain text extensions (visible text), and the one
//! application extension that sets the loop count. Comments and every other
//! application extension — XMP rides in one — leave, as do bytes after the
//! trailer.

use super::{add_rule, RasterMetadataRule, RasterStripError, StripResult};

const GIF_HEADERS: [&[u8]; 2] = [b"GIF87a", b"GIF89a"];
/// Six signature bytes, then width, height, the packed flags, background
/// index and aspect ratio.
const SCREEN_FLAGS_AT: usize = 10;
const SCREEN_DESCRIPTOR_END: usize = 13;
const IMAGE_DESCRIPTOR_LEN: usize = 10;

const EXTENSION: u8 = 0x21;
const IMAGE: u8 = 0x2c;
const TRAILER: u8 = 0x3b;

const PLAIN_TEXT_LABEL: u8 = 0x01;
const GRAPHIC_CONTROL_LABEL: u8 = 0xf9;
const COMMENT_LABEL: u8 = 0xfe;
const APPLICATION_LABEL: u8 = 0xff;

/// The eight-byte application identifier and the three-byte authentication
/// code, which the specification puts in one eleven-byte sub-block.
const APPLICATION_ID_LEN: usize = 11;
const XMP_APPLICATION: &[u8] = b"XMP DataXMP";
const LOOPING_APPLICATIONS: [&[u8]; 2] = [b"NETSCAPE2.0", b"ANIMEXTS1.0"];

pub(super) fn has_gif_header(bytes: &[u8]) -> bool {
    GIF_HEADERS.iter().any(|header| bytes.starts_with(header))
}

pub(super) fn strip_gif_blocks(bytes: &[u8], removed: &mut Vec<RasterMetadataRule>) -> StripResult {
    if !has_gif_header(bytes) {
        return Err(RasterStripError::new(
            "it does not begin with a GIF87a or GIF89a header".to_string(),
        ));
    }
    if bytes.len() < SCREEN_DESCRIPTOR_END {
        return Err(RasterStripError::new(
            "the logical screen descriptor is truncated".to_string(),
        ));
    }
    let mut index = SCREEN_DESCRIPTOR_END + color_table_len(bytes[SCREEN_FLAGS_AT]);
    if index > bytes.len() {
        return Err(RasterStripError::new(
            "the global colour table runs past the end of the file".to_string(),
        ));
    }
    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(&bytes[..index]);

    loop {
        let Some(&introducer) = bytes.get(index) else {
            return Err(RasterStripError::new(
                "it ends without a trailer".to_string(),
            ));
        };
        match introducer {
            TRAILER => {
                out.push(TRAILER);
                index += 1;
                break;
            }
            IMAGE => {
                let end = image_block_end(bytes, index)?;
                out.extend_from_slice(&bytes[index..end]);
                index = end;
            }
            EXTENSION => {
                let Some(&label) = bytes.get(index + 1) else {
                    return Err(RasterStripError::new(format!(
                        "the extension at byte {index} has no label"
                    )));
                };
                let end = skip_sub_blocks(bytes, index + 2)?;
                if keeps_extension(label, &bytes[index + 2..end], removed)? {
                    out.extend_from_slice(&bytes[index..end]);
                }
                index = end;
            }
            other => {
                return Err(RasterStripError::new(format!(
                    "byte {index} is {other:#04x}, which introduces no GIF block"
                )));
            }
        }
    }

    if index != bytes.len() {
        add_rule(removed, RasterMetadataRule::TrailingData);
    }
    Ok(out)
}

/// Bytes of a colour table whose size the packed flags name: none, or
/// `3 * 2^(n + 1)`.
fn color_table_len(packed: u8) -> usize {
    if packed & 0x80 == 0 {
        0
    } else {
        3usize << ((packed & 0x07) + 1)
    }
}

/// Where the image block at `start` ends: its descriptor, local colour table,
/// the minimum code size byte and the sub-block chain of LZW data.
fn image_block_end(bytes: &[u8], start: usize) -> Result<usize, RasterStripError> {
    let Some(&local_flags) = bytes.get(start + IMAGE_DESCRIPTOR_LEN - 1) else {
        return Err(RasterStripError::new(format!(
            "the image descriptor at byte {start} is truncated"
        )));
    };
    let data = start + IMAGE_DESCRIPTOR_LEN + color_table_len(local_flags) + 1;
    if data > bytes.len() {
        return Err(RasterStripError::new(format!(
            "the image at byte {start} runs past the end of the file before its data"
        )));
    }
    skip_sub_blocks(bytes, data)
}

/// The offset just past the zero-length terminator of the sub-block chain that
/// begins at `start`.
fn skip_sub_blocks(bytes: &[u8], start: usize) -> Result<usize, RasterStripError> {
    let mut at = start;
    loop {
        let Some(&length) = bytes.get(at) else {
            return Err(RasterStripError::new(format!(
                "the sub-block chain starting at byte {start} never terminates"
            )));
        };
        at += 1;
        if length == 0 {
            return Ok(at);
        }
        at += usize::from(length);
        if at > bytes.len() {
            return Err(RasterStripError::new(format!(
                "a sub-block in the chain starting at byte {start} runs past the end of the file"
            )));
        }
    }
}

/// Whether the extension survives; a dropped one is recorded. `chain` is its
/// sub-blocks, terminator included.
///
/// A graphic control extension is the one fixed shape the format defines, so
/// anything else under that label is refused rather than kept as a carrier.
/// A looping extension survives only in the shape that sets a loop count: any
/// other bytes under its name would be a payload in a trusted wrapper.
fn keeps_extension(
    label: u8,
    chain: &[u8],
    removed: &mut Vec<RasterMetadataRule>,
) -> Result<bool, RasterStripError> {
    match label {
        GRAPHIC_CONTROL_LABEL => {
            if chain.len() == 6 && chain[0] == 4 && chain[5] == 0 {
                Ok(true)
            } else {
                Err(RasterStripError::new(
                    "its graphic control extension is not the four-byte block the format defines"
                        .to_string(),
                ))
            }
        }
        PLAIN_TEXT_LABEL => Ok(true),
        COMMENT_LABEL => {
            add_rule(removed, RasterMetadataRule::Comment);
            Ok(false)
        }
        APPLICATION_LABEL => {
            let identifier = chain
                .get(1..=APPLICATION_ID_LEN)
                .filter(|_| usize::from(chain[0]) == APPLICATION_ID_LEN);
            if let Some(identifier) = identifier {
                if LOOPING_APPLICATIONS.contains(&identifier)
                    && is_loop_count(&chain[1 + APPLICATION_ID_LEN..])
                {
                    return Ok(true);
                }
            }
            let rule = if identifier == Some(XMP_APPLICATION) {
                RasterMetadataRule::Xmp
            } else {
                RasterMetadataRule::UnknownChunk
            };
            add_rule(removed, rule);
            Ok(false)
        }
        _ => {
            add_rule(removed, RasterMetadataRule::UnknownChunk);
            Ok(false)
        }
    }
}

/// One three-byte sub-block, its first byte `1`, then the terminator.
fn is_loop_count(rest: &[u8]) -> bool {
    rest.len() == 5 && rest[0] == 3 && rest[1] == 1 && rest[4] == 0
}
