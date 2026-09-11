//! Taking identity out of a raster attachment before the daemon forwards it.
//!
//! `src/features/design/rasterMetadata.ts` does this in the browser, at import
//! time. This module is the same rule in Rust inside the daemon, and it is not
//! here for symmetry.
//!
//! The composer is one entrance. An attachment can now arrive from another
//! device — a photo attached on a phone, read by the daemon on the PC — or
//! forwarded by the relay, and neither path passes through the composer.
//! Coordinates travelling between the owner's own devices are not the leak. The
//! leak is daemon to provider, where the bytes leave the house, and every path
//! crosses that hop. That is the only place a guarantee is a guarantee, so it
//! is the place this rule is stated a second time. The frontend copy exists to
//! tell the designer what left their file while they attach it; the daemon has
//! no window and can tell them nothing, so this one exists to make the strip
//! hold for bytes the composer never saw.
//!
//! Three properties carry the whole rule, and each of them is a choice against
//! the obvious alternative.
//!
//! **The surviving set is a whitelist, never a denylist.** Of the sixteen JPEG
//! `APPn` segments three are named to survive — an `APP0` whose payload is
//! `JFIF\0`, an `APP14` (Adobe's colour transform), and an `APP2` whose payload
//! opens with `ICC_PROFILE\0`, because those say how to decode the pixels
//! rather than who produced them. The PNG side names its surviving chunks one
//! by one. A denylist removes only what someone thought to name; everything
//! absent from a whitelist leaves *because it was not named*, so a vendor block
//! this code has never seen goes with the rest and no rule has to recognise it
//! first. Each JPEG survivor is identified by its payload rather than by its
//! marker number: an `APP0` carrying `JFXX\0` is an embedded thumbnail, a
//! second image inside the file, and an `APP2` that is not an ICC profile is a
//! vendor block by definition.
//!
//! **Surviving bytes are copied verbatim. No CRC is ever recomputed.** A
//! recomputed CRC would ratify an accidental edit to the bytes it covers, and
//! the corruption would come out looking valid.
//!
//! **A file that cannot be walked is refused, never returned.** Handing the
//! input back on a parse failure would re-admit exactly the bytes the caller
//! asked to have removed and would report a sanitizer that did nothing as a
//! success. Turning a failure into an apparently clean attachment is worse than
//! a rejection the caller can see.
//!
//! The JPEG entropy-coded scan is traversed by the stuffing rules rather than
//! skipped, because the structure of that data is what says where it ends. That
//! is how bytes after `EOI` are found and dropped, and how an `APPn` sitting
//! between two scans of a progressive JPEG gets examined at all. Traversing is
//! not rewriting: what the walk produces is a length, and the bytes of the scan
//! are copied from the input.
//!
//! The vocabulary is shared. [`RasterMetadataRule`] carries the same nine names
//! as `RASTER_METADATA_RULES` in the TypeScript module, and the bytes the two
//! implementations must agree on live in `fixtures/raster-metadata/vectors.json`
//! at the repo root — outside both source trees, because a fixture inside either
//! one reads as that side's property and gets edited to match that side's
//! behaviour. The test below reads it through `include_str!`, so the dependency
//! is a compile-time one: moving or deleting the file stops the daemon building
//! instead of letting the two implementations drift apart in silence.
//!
//! What is deliberately *not* ported is the user-facing half of the TypeScript
//! module: the `label`/`reason` table and `rasterMetadataNotice`. Those exist to
//! write one sentence for a designer in the composer. The daemon has no window,
//! its refusals become a `WireError`, and porting a sentence nobody reads would
//! add a second place for the same nine names to be misspelled.

use serde::{Serialize, Serializer};

/// The nine things this pass can take out of a file.
///
/// The names this serialises to are the same strings `RASTER_METADATA_RULES`
/// uses in TypeScript, and the vectors file names its expectations with them.
/// A rename here is therefore a change to the shared vocabulary, not an
/// internal refactor: the TypeScript test asserts that the vectors cover every
/// rule in its own table, and this side asserts the same against [`Self::ALL`],
/// so the two tables stay pinned to the same nine names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RasterMetadataRule {
    /// EXIF, and any `APPn` whose payload does not identify itself as one of
    /// the named survivors.
    Exif,
    /// Adobe's XMP packet, which repeats EXIF's coordinates as XML.
    Xmp,
    /// An `APP13` IPTC/Photoshop block.
    Iptc,
    /// A JPEG `COM` segment: free text an editor or a camera left behind, with
    /// no defined role in rendering and room in it to name a person or a
    /// machine. Declared here, between `Iptc` and `TextChunk`, because the
    /// declaration order is the order `RASTER_METADATA_RULES` declares its
    /// names in TypeScript and therefore the sort order a `removed` list
    /// compares in. A JPEG that loses both a `COM` segment and something else
    /// would otherwise disagree with its vector for a reason that has nothing
    /// to do with the bytes.
    Comment,
    /// A PNG `tEXt`, `zTXt` or `iTXt` chunk. Not the JPEG side: a `COM` segment
    /// is [`Self::Comment`], which the TypeScript table carries as a separate
    /// name with a separate reason.
    TextChunk,
    /// A PNG `tIME` chunk.
    Timestamp,
    /// A PNG `dSIG` chunk.
    Signature,
    /// Bytes after the JPEG `EOI` marker or the PNG `IEND` chunk.
    TrailingData,
    /// A PNG chunk this pass does not keep and does not know by name, dropped
    /// because the PNG specification marks it ancillary.
    UnknownChunk,
}

/// The rule names are the port's own statement of its vocabulary, which is what
/// the shared vectors are checked against, so the list and the naming function
/// stay together. A build without tests has no reader for either — the walk
/// itself only needs to know whether a rule has fired, not what it is called —
/// so the block is marked test-facing for a non-test build, the same way
/// `acp_view` and `acp_client` mark theirs, rather than being split out and
/// leaving the production build with no vocabulary to declare.
#[cfg_attr(not(test), allow(dead_code))]
impl RasterMetadataRule {
    /// Every rule, in declaration order.
    pub(crate) const ALL: [Self; 9] = [
        Self::Exif,
        Self::Xmp,
        Self::Iptc,
        Self::Comment,
        Self::TextChunk,
        Self::Timestamp,
        Self::Signature,
        Self::TrailingData,
        Self::UnknownChunk,
    ];

    /// The name this rule carries in the vectors file and in the composer's
    /// notice.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Exif => "exif",
            Self::Xmp => "xmp",
            Self::Iptc => "iptc",
            Self::Comment => "comment",
            Self::TextChunk => "textChunk",
            Self::Timestamp => "timestamp",
            Self::Signature => "signature",
            Self::TrailingData => "trailingData",
            Self::UnknownChunk => "unknownChunk",
        }
    }
}

/// The rule's name is the string the shared vectors and the composer's notice
/// use, so serialising a rule is the same act as naming it. Written by hand
/// rather than derived: a `#[serde(rename = ...)]` on each variant would put
/// the same string in two places and let a variant be renamed without its wire
/// name moving with it, which is exactly the drift the vectors exist to catch.
impl Serialize for RasterMetadataRule {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Why a raster could not be walked.
///
/// The sentence is the one the TypeScript pass produces for the same bytes,
/// addressed to the caller that has to decide what to say next. It is
/// deliberately not the wire message: that is built where the refusal becomes a
/// `WireError`, and the two have different readers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RasterStripError {
    reason: String,
}

impl RasterStripError {
    fn new(reason: String) -> Self {
        Self { reason }
    }
}

impl std::fmt::Display for RasterStripError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

/// Which container the caller has already decided these bytes are.
///
/// The measured MIME decides the parse; a caller that has not sniffed the bytes
/// should not be calling this at all, which is why this is an enum and not the
/// string that is on the wire. `image/svg+xml` deliberately has no variant: see
/// `attachment_store::SessionAttachments::materialize` for why SVG is a named
/// gap rather than half a guarantee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RasterMime {
    Jpeg,
    Png,
}

impl RasterMime {
    pub(crate) fn from_mime_type(mime_type: &str) -> Option<Self> {
        match mime_type {
            "image/jpeg" => Some(Self::Jpeg),
            "image/png" => Some(Self::Png),
            _ => None,
        }
    }
}

/// Which container these bytes actually are, read from their leading bytes and
/// nothing else, or `None` when they are neither.
///
/// This exists because [`RasterMime::from_mime_type`] decides from a string
/// that arrived over the wire. The declared type is the sender's word for what
/// they sent; these bytes are the evidence, and where the two disagree the
/// evidence is the one that decides what the walk is allowed to be applied to.
/// Without this, the label would choose whether the guarantee ran at all, and
/// the label belongs to whoever sent the bytes.
///
/// The three-byte JPEG prefix is deliberate rather than a two-byte one. Every
/// marker after `SOI` in a well-formed JPEG is introduced by `FF`, so a real
/// JPEG always has that third byte; a file that stops at `FF D8` is not an
/// image this pass could walk in any case. Nothing else is recognised here on
/// purpose: a pass that guessed a container from a byte it did not know would
/// be inventing a rule rather than applying one.
pub(crate) fn sniff_raster_mime(bytes: &[u8]) -> Option<RasterMime> {
    if bytes.starts_with(&JPEG_SIGNATURE) {
        return Some(RasterMime::Jpeg);
    }
    if bytes.starts_with(&PNG_SIGNATURE) {
        return Some(RasterMime::Png);
    }
    None
}

/// A file this pass walked, with the rules that fired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StrippedRaster {
    /// The bytes to write: the input with the named things removed, or a copy
    /// of the input when no rule fired.
    pub(crate) bytes: Vec<u8>,
    /// The rules that fired, first occurrence first, each named once.
    ///
    /// This is the walk's observable output and what the shared vectors assert,
    /// which is why it travels rather than being thrown away. The daemon itself
    /// reads only `bytes` — it has no window to put a sentence in — so a build
    /// without tests appears not to read this field.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) removed: Vec<RasterMetadataRule>,
}

type StripResult = Result<Vec<u8>, RasterStripError>;

/// Returns the same image with its identity metadata removed, or an error
/// saying why the bytes could not be walked.
///
/// When no rule fires the input comes back unchanged rather than rebuilt, so an
/// image that carried nothing is not rewritten for no reason.
pub(crate) fn strip_raster_metadata(
    bytes: &[u8],
    mime: RasterMime,
) -> Result<StrippedRaster, RasterStripError> {
    let mut removed: Vec<RasterMetadataRule> = Vec::new();
    let stripped = match mime {
        RasterMime::Png => strip_png_chunks(bytes, &mut removed)?,
        RasterMime::Jpeg => strip_jpeg_segments(bytes, &mut removed)?,
    };
    Ok(StrippedRaster {
        bytes: stripped,
        removed,
    })
}

/// Record a rule once, keeping the order it first fired in.
fn add_rule(removed: &mut Vec<RasterMetadataRule>, rule: RasterMetadataRule) {
    if !removed.contains(&rule) {
        removed.push(rule);
    }
}

/// `SOI` followed by the `FF` that introduces the first marker.
///
/// Three bytes rather than two: every marker after `SOI` in a well-formed JPEG
/// is introduced by `FF`, so a real JPEG always carries the third byte. A file
/// that stops at `FF D8` is not an image either walk can follow, which is what
/// lets the sniff decline to name it rather than guessing.
const JPEG_SIGNATURE: [u8; 3] = [0xff, 0xd8, 0xff];
const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const PNG_IEND: &[u8] = b"IEND";
const JFIF_PREFIX: &[u8] = b"JFIF\0";
const ICC_PROFILE_PREFIX: &[u8] = b"ICC_PROFILE\0";
const XMP_PREFIX: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// Whether `prefix` sits at `at`, without reading past the end of `bytes`.
fn matches_at(bytes: &[u8], at: usize, prefix: &[u8]) -> bool {
    match bytes
        .get(at..)
        .and_then(|remaining| remaining.get(..prefix.len()))
    {
        Some(window) => window == prefix,
        None => false,
    }
}

/// The PNG chunks that survive, named one by one.
///
/// Everything absent from this set is decided by the PNG specification rather
/// than by a guess: the fifth bit of the first byte of the chunk type separates
/// critical chunks from ancillary ones, and the specification requires a
/// decoder to draw an image while ignoring every ancillary chunk it does not
/// understand.
///
/// That guarantee is about *decoding*, not about *rendering*, which is why this
/// is not simply "the four critical chunks". An image still decodes without its
/// colour chunks; it comes out the wrong colour. So every chunk that changes
/// what the eye sees is named here, one by one, or the strip quietly becomes an
/// edit. `IHDR`, `PLTE`, `IDAT` and `IEND` are the image. `tRNS`, `gAMA`,
/// `cHRM`, `sRGB`, `iCCP`, `sBIT`, `bKGD`, `hIST`, `pHYs` and `sPLT` all change
/// how the pixels are rendered. `cICP`, `mDCv` and `cLLi` are the third-edition
/// HDR colour chunks, and an image rendered without them shows different
/// colours on a wide-gamut display. `acTL`, `fcTL` and `fdAT` are an APNG's
/// animation control and frame data: ancillary, so a whitelist that forgot them
/// would leave a valid, still PNG that no longer moves.
const PNG_KEPT_CHUNKS: [&str; 20] = [
    "IHDR", "PLTE", "IDAT", "IEND", "tRNS", "gAMA", "cHRM", "sRGB", "iCCP", "sBIT", "bKGD", "hIST",
    "pHYs", "sPLT", "cICP", "mDCv", "cLLi", "acTL", "fcTL", "fdAT",
];

/// The identity chunks, named to the rule each one fires.
const PNG_METADATA_CHUNKS: [(&str, RasterMetadataRule); 6] = [
    ("eXIf", RasterMetadataRule::Exif),
    ("tEXt", RasterMetadataRule::TextChunk),
    ("zTXt", RasterMetadataRule::TextChunk),
    ("iTXt", RasterMetadataRule::TextChunk),
    ("tIME", RasterMetadataRule::Timestamp),
    ("dSIG", RasterMetadataRule::Signature),
];

fn png_kept_chunk(chunk_type: &[u8]) -> bool {
    PNG_KEPT_CHUNKS
        .iter()
        .any(|kept| kept.as_bytes() == chunk_type)
}

fn png_metadata_rule(chunk_type: &[u8]) -> Option<RasterMetadataRule> {
    PNG_METADATA_CHUNKS
        .iter()
        .find(|entry| entry.0.as_bytes() == chunk_type)
        .map(|entry| entry.1)
}

/// JPEG: from `SOI`, marker by marker. Every marker is `FF xx`; the markers
/// that carry a payload have a two-byte big-endian length that counts itself.
///
/// The walk reads the entropy-coded data, and reading it is not the same as
/// rewriting it. Byte stuffing is a defined structure: inside a scan `FF 00` is
/// a literal `FF`, `FF D0`-`FF D7` are restart markers and `FF FF` is padding,
/// so an `FF` that announces a real marker is unambiguous. Walking it is how
/// the end of a scan is found — and it has to be found, because a progressive
/// JPEG interleaves several scans with segment markers between them, and a
/// segment there would otherwise never be examined at all. What the walk
/// produces is only a length: the bytes of the scan, from the `SOS` header to
/// the marker that closes it, are copied verbatim, stuffing and restart markers
/// included.
///
/// The first real marker after a scan is either `EOI`, where the image ends, or
/// the next segment, where the marker loop resumes. Bytes after `EOI` are not
/// part of the image and are dropped, because a strip that reads that far and
/// then ships them anyway keeps open the same hiding place it exists to close.
fn strip_jpeg_segments(bytes: &[u8], removed: &mut Vec<RasterMetadataRule>) -> StripResult {
    if bytes.len() < 2 || bytes[0] != 0xff || bytes[1] != 0xd8 {
        return Err(RasterStripError::new(
            "it does not begin with the JPEG start-of-image marker FF D8".to_string(),
        ));
    }
    let mut out = vec![0u8; bytes.len()];
    out[0] = 0xff;
    out[1] = 0xd8;
    let mut written = 2usize;
    let mut index = 2usize;
    let mut saw_scan = false;

    while index < bytes.len() {
        if index + 1 >= bytes.len() {
            return Err(RasterStripError::new(format!(
                "the marker at byte {index} is truncated: the file ends after FF"
            )));
        }
        if bytes[index] != 0xff {
            return Err(RasterStripError::new(format!(
                "byte {index} is 0x{:x}, not the FF that begins a marker",
                bytes[index]
            )));
        }
        let code = bytes[index + 1];
        // A run of FF is padding before the real marker; carry it through one
        // byte at a time so the next round can read the marker that follows.
        if code == 0xff {
            out[written] = 0xff;
            written += 1;
            index += 1;
            continue;
        }
        if code == 0x00 {
            return Err(RasterStripError::new(format!(
                "byte {index} begins FF 00 outside the entropy-coded data, where it is not a marker"
            )));
        }
        if code == 0xd8 {
            return Err(RasterStripError::new(
                "it contains a second start-of-image marker".to_string(),
            ));
        }
        if code == 0xd9 {
            if !saw_scan {
                return Err(RasterStripError::new(
                    "it ends before its first scan (no SOS marker)".to_string(),
                ));
            }
            out[written] = 0xff;
            out[written + 1] = 0xd9;
            written += 2;
            index += 2;
            if index < bytes.len() {
                add_rule(removed, RasterMetadataRule::TrailingData);
            }
            break;
        }
        // RST0-RST7 and TEM have no length field; reading one would consume the
        // two bytes after them as a length and walk off into the next marker.
        if code == 0x01 || (0xd0..=0xd7).contains(&code) {
            out[written] = 0xff;
            out[written + 1] = code;
            written += 2;
            index += 2;
            continue;
        }
        if index + 3 >= bytes.len() {
            return Err(RasterStripError::new(format!(
                "the segment at byte {index} is truncated before its length field"
            )));
        }
        let length = (usize::from(bytes[index + 2]) << 8) | usize::from(bytes[index + 3]);
        if length < 2 {
            return Err(RasterStripError::new(format!(
                "the segment at byte {index} declares length {length}, which cannot be less than 2"
            )));
        }
        // `length` is 16 bits and `index` indexes a slice that already exists,
        // so this sum cannot overflow a `usize` on any target — unlike the PNG
        // side, where the declared length is 32 bits and the same sum needs a
        // wider type on a 32-bit target to stay provably safe.
        let segment_end = index + 2 + length;
        if segment_end > bytes.len() {
            return Err(RasterStripError::new(format!(
                "the segment at byte {index} declares {length} bytes but only {} are left in the file",
                bytes.len() - index - 2
            )));
        }
        if code == 0xda {
            // Start of scan. The header is copied now; the entropy-coded data
            // goes in once the walk below has found where it ends.
            out[written..written + (segment_end - index)]
                .copy_from_slice(&bytes[index..segment_end]);
            written += segment_end - index;
            let mut scan = segment_end;
            let mut closed_by_marker = false;
            while scan < bytes.len() {
                if bytes[scan] != 0xff {
                    scan += 1;
                    continue;
                }
                if scan + 1 >= bytes.len() {
                    // A lone FF as the last byte begins no marker, so reading
                    // past it would leave the buffer. It is a truncation.
                    return Err(RasterStripError::new(
                        "it ends on a lone FF byte inside the entropy-coded data, which begins no marker"
                            .to_string(),
                    ));
                }
                let next = bytes[scan + 1];
                if next == 0x00 {
                    // FF 00 is a literal FF in the scan, not a marker.
                    scan += 2;
                    continue;
                }
                if next == 0xff {
                    // Fill byte before the real marker; step over it.
                    scan += 1;
                    continue;
                }
                if (0xd0..=0xd7).contains(&next) {
                    // RST0-RST7 restart the entropy coder inside the scan and
                    // do not end it.
                    scan += 2;
                    continue;
                }
                closed_by_marker = true;
                break;
            }
            if !closed_by_marker {
                return Err(RasterStripError::new(
                    "it finishes inside the entropy-coded data of a scan without an EOI marker"
                        .to_string(),
                ));
            }
            // The scan's bytes are copied verbatim; the walk only measured it.
            out[written..written + (scan - segment_end)].copy_from_slice(&bytes[segment_end..scan]);
            written += scan - segment_end;
            index = scan;
            saw_scan = true;
            // A progressive JPEG resumes the marker loop here, which is also
            // what lets an APPn between two scans be examined rather than
            // skipped.
            continue;
        }
        let payload = index + 4;
        if (0xe0..=0xef).contains(&code) {
            // APPn: keep the three named survivors, drop everything else.
            let survives = (code == 0xe0 && matches_at(bytes, payload, JFIF_PREFIX))
                || code == 0xee
                || (code == 0xe2 && matches_at(bytes, payload, ICC_PROFILE_PREFIX));
            if !survives {
                if code == 0xed {
                    add_rule(removed, RasterMetadataRule::Iptc);
                } else if matches_at(bytes, payload, XMP_PREFIX) {
                    add_rule(removed, RasterMetadataRule::Xmp);
                } else {
                    add_rule(removed, RasterMetadataRule::Exif);
                }
                index = segment_end;
                continue;
            }
        }
        if code == 0xfe {
            add_rule(removed, RasterMetadataRule::Comment);
            index = segment_end;
            continue;
        }
        // Everything else — a kept APP0, APP14, an ICC APP2, DQT, DHT, SOF*,
        // DRI — is copied byte for byte.
        out[written..written + (segment_end - index)].copy_from_slice(&bytes[index..segment_end]);
        written += segment_end - index;
        index = segment_end;
    }

    if !saw_scan {
        return Err(RasterStripError::new(
            "it ends without a start-of-scan marker".to_string(),
        ));
    }
    if removed.is_empty() {
        return Ok(bytes.to_vec());
    }
    Ok(out[..written].to_vec())
}

/// PNG: the eight-byte signature, then chunks of `length | type | data | CRC`.
///
/// A chunk that survives is copied with its own CRC still attached. A chunk the
/// pass knows is identity leaves under its own rule. Any other chunk is decided
/// by the critical bit: ancillary ones go, and a critical one makes the whole
/// file unreadable. The whitelist is what makes a private chunk (`prVW`,
/// `mkBF`, anything registered later) leave without this code having to know
/// the type, exactly as the JPEG side does for `APPn`.
fn strip_png_chunks(bytes: &[u8], removed: &mut Vec<RasterMetadataRule>) -> StripResult {
    if bytes.len() < 8 || bytes[..8] != PNG_SIGNATURE[..] {
        return Err(RasterStripError::new(
            "it does not begin with the eight-byte PNG signature".to_string(),
        ));
    }
    let mut out = vec![0u8; bytes.len()];
    out[..8].copy_from_slice(&PNG_SIGNATURE);
    let mut written = 8usize;
    let mut index = 8usize;
    let mut saw_end = false;

    while index < bytes.len() {
        if index + 8 > bytes.len() {
            return Err(RasterStripError::new(format!(
                "the chunk header at byte {index} is truncated"
            )));
        }
        let length = u32::from_be_bytes([
            bytes[index],
            bytes[index + 1],
            bytes[index + 2],
            bytes[index + 3],
        ]);
        let chunk_type = &bytes[index + 4..index + 8];
        // u64 so the arithmetic cannot wrap whatever the declared length is;
        // the comparison against the real length is what makes it exact.
        let chunk_end = index as u64 + 8 + u64::from(length) + 4;
        if chunk_end > bytes.len() as u64 {
            return Err(RasterStripError::new(format!(
                "the {} chunk at byte {index} declares {length} data bytes and runs past the end of the file",
                String::from_utf8_lossy(chunk_type)
            )));
        }
        let chunk_end = chunk_end as usize;
        if png_kept_chunk(chunk_type) {
            out[written..written + (chunk_end - index)].copy_from_slice(&bytes[index..chunk_end]);
            written += chunk_end - index;
        } else if let Some(rule) = png_metadata_rule(chunk_type) {
            add_rule(removed, rule);
        } else {
            // The critical bit: the fifth bit of the first byte of the type. A
            // lowercase first letter means ancillary, and the specification
            // requires a decoder to be able to ignore those, so dropping one
            // cannot change the picture. That is a property of the format, not
            // a judgement about the contents.
            let ancillary = chunk_type[0] & 0x20 != 0;
            if !ancillary {
                return Err(RasterStripError::new(format!(
                    "it carries a critical {} chunk, which this pass does not know how to keep or remove",
                    String::from_utf8_lossy(chunk_type)
                )));
            }
            add_rule(removed, RasterMetadataRule::UnknownChunk);
        }
        index = chunk_end;
        if chunk_type == PNG_IEND {
            saw_end = true;
            break;
        }
    }

    if !saw_end {
        return Err(RasterStripError::new(
            "it ends without an IEND chunk".to_string(),
        ));
    }
    if index != bytes.len() {
        // Bytes after IEND are not a chunk and cannot be read as one: a
        // metadata blob appended after the image is a known hiding place. They
        // are dropped rather than refused — the picture up to IEND is whole and
        // draws identically, and rejecting the file would cost the user an
        // image over bytes nobody needed.
        add_rule(removed, RasterMetadataRule::TrailingData);
    }
    if removed.is_empty() {
        return Ok(bytes.to_vec());
    }
    Ok(out[..written].to_vec())
}

/// A synthetic container for tests, and only for tests.
///
/// The real fixtures are the shared vectors; this exists because the code paths
/// that *call* the strip (the attachment store, and the session send that
/// reaches it) need bytes the walk accepts, and hand-writing a PNG in each test
/// module would let the two copies drift. The CRC fields carry filler: the walk
/// never reads a CRC and never recomputes one, so a filler is indistinguishable
/// from a real one here.
///
/// `idat` varies the encoded pixel so two calls produce two different files.
#[cfg(test)]
pub(crate) fn clean_png(idat: u8) -> Vec<u8> {
    let mut out = PNG_SIGNATURE.to_vec();
    push_png_chunk(&mut out, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
    push_png_chunk(
        &mut out,
        b"IDAT",
        &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, idat],
    );
    push_png_chunk(&mut out, b"IEND", &[]);
    out
}

/// The same container with a `tEXt` chunk, which the walk removes. Cutting this
/// chunk out of the result is exactly `clean_png(0x01)`.
#[cfg(test)]
pub(crate) fn png_with_text_chunk() -> Vec<u8> {
    let mut out = PNG_SIGNATURE.to_vec();
    push_png_chunk(&mut out, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
    push_png_chunk(&mut out, b"tEXt", b"note\0shot on a phone");
    push_png_chunk(
        &mut out,
        b"IDAT",
        &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
    );
    push_png_chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
fn push_png_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(data);
    out.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
}

/// The shared vectors, bound at compile time on purpose: `include_str!` makes
/// the dependency a build-time one, so moving or deleting the file stops the
/// daemon building instead of letting the two implementations drift apart in
/// silence. The path is relative to this file.
///
/// It sits outside `mod tests` because the code that *calls* the walk has tests
/// of its own, and those need real containers rather than hand-rolled bytes:
/// a second copy of a JPEG written for one test module would drift from the
/// bytes the vectors assert on, which is the drift this file exists to prevent.
#[cfg(test)]
const VECTORS_JSON: &str = include_str!("../../../fixtures/raster-metadata/vectors.json");

/// Lowercase hex, no separators. A vector written in base64 or in `0x` form
/// would still parse and would then differ from what the Rust side reads, which
/// is the one failure this arrangement exists to make impossible.
#[cfg(test)]
fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
        return Err(format!("an odd number of hex digits: {hex:?}"));
    }
    if !hex
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("not lowercase hex: {hex:?}"));
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for index in (0..hex.len()).step_by(2) {
        bytes.push(
            u8::from_str_radix(&hex[index..index + 2], 16)
                .map_err(|error| format!("{hex:?}: {error}"))?,
        );
    }
    Ok(bytes)
}

/// One field of one vector, as the hex string the file carries.
///
/// Read through `serde_json::Value` rather than the typed structs the tests
/// below declare, because this helper is reached from another module and a
/// helper that wants one field should not have to reproduce the whole shape.
#[cfg(test)]
fn vector_hex(name: &str, expected_output: bool) -> String {
    let file: serde_json::Value =
        serde_json::from_str(VECTORS_JSON).expect("the shared vectors file must parse");
    let vectors = file["vectors"]
        .as_array()
        .expect("the vectors file carries an array of vectors");
    let vector = vectors
        .iter()
        .find(|entry| entry.get("name").and_then(|value| value.as_str()) == Some(name))
        .unwrap_or_else(|| panic!("the vectors file has no vector named {name:?}"));
    let field: &serde_json::Value = if expected_output {
        &vector["expect"]["output"]
    } else {
        &vector["input"]
    };
    field
        .as_str()
        .unwrap_or_else(|| panic!("{name:?} carries no hex string at that field"))
        .to_string()
}

/// A passing vector's input bytes, for a test outside this module that needs a
/// real container instead of bytes it made up. Panics if the vector is absent
/// or is one of the refused ones.
#[cfg(test)]
pub(crate) fn vector_input(name: &str) -> Vec<u8> {
    decode_hex(&vector_hex(name, false)).expect("a vector's input is lowercase hex")
}

/// The same vector's expected output bytes: its input with the named things
/// removed, byte for byte. This is what a test asserting the caller wrote the
/// right file compares against, so that it is comparing to the shared rule
/// rather than to whatever the implementation happened to produce.
#[cfg(test)]
pub(crate) fn vector_output(name: &str) -> Vec<u8> {
    decode_hex(&vector_hex(name, true)).expect("a vector's output is lowercase hex")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct VectorsFile {
        note: String,
        vectors: Vec<Vector>,
    }

    #[derive(Deserialize)]
    struct Vector {
        name: String,
        mime: String,
        input: String,
        expect: Expectation,
    }

    /// A passing vector names the output bytes and the rules that fired; a
    /// refused one carries `ok: false` and nothing else. Both shapes land in
    /// this one struct rather than in an untagged enum, so the test can see an
    /// unexpected field instead of having the shape silently mismatched.
    #[derive(Deserialize)]
    struct Expectation {
        ok: bool,
        output: Option<String>,
        removed: Option<Vec<String>>,
    }

    fn vectors() -> VectorsFile {
        serde_json::from_str(VECTORS_JSON).expect("the shared vectors file must parse")
    }

    fn mime_of(name: &str) -> RasterMime {
        RasterMime::from_mime_type(name).unwrap_or_else(|| panic!("unexpected mime type {name}"))
    }

    /// The file's own integrity, before any implementation is compared to it.
    ///
    /// A file that lost half its entries would otherwise pass the test below
    /// silently, which is the failure mode the file exists to prevent.
    #[test]
    fn the_shared_vectors_file_is_intact() {
        let file = vectors();
        assert!(
            !file.vectors.is_empty(),
            "the vectors file must not be empty"
        );
        assert!(
            !file.note.is_empty(),
            "the vectors file must carry its note"
        );

        let mut containers: Vec<&str> = file.vectors.iter().map(|v| v.mime.as_str()).collect();
        containers.sort();
        containers.dedup();
        assert_eq!(
            containers,
            vec!["image/jpeg", "image/png"],
            "the vectors must cover both containers"
        );

        let mut covered: Vec<String> = Vec::new();
        for vector in &file.vectors {
            if !vector.expect.ok {
                // The two sides' failures have different audiences — a sentence
                // shown to the designer in the composer against a WireError on
                // the daemon's pipe — and pinning the wording would force one of
                // them to read the other's sentence. The rule is restated in the
                // file's own note.
                assert!(
                    vector.expect.output.is_none() && vector.expect.removed.is_none(),
                    "{}: a refusal asserts ok: false and nothing more",
                    vector.name
                );
                continue;
            }
            let removed = vector
                .expect
                .removed
                .as_ref()
                .unwrap_or_else(|| panic!("{}: a passing vector names its rules", vector.name));
            assert!(
                vector.expect.output.is_some(),
                "{}: a passing vector names its output",
                vector.name
            );
            covered.extend(removed.iter().cloned());
        }
        covered.sort();
        covered.dedup();

        // Equality rather than containment, in both directions on purpose. A
        // new rule added to the table without a vector fails here, because the
        // Rust side has to learn the new vocabulary and not merely the bytes it
        // already agreed on; a vector naming a rule that no longer exists fails
        // here too.
        let mut rules: Vec<String> = RasterMetadataRule::ALL
            .iter()
            .map(|rule| rule.as_str().to_string())
            .collect();
        rules.sort();
        assert_eq!(
            covered, rules,
            "the vectors must exercise every rule name, and invent none"
        );

        for mime in ["image/jpeg", "image/png"] {
            let clean = file.vectors.iter().any(|vector| {
                vector.mime == mime
                    && vector.expect.ok
                    && vector
                        .expect
                        .removed
                        .as_ref()
                        .is_some_and(|removed| removed.is_empty())
            });
            assert!(clean, "{mime} has no vector that removes nothing");
        }
    }

    #[test]
    fn every_vector_agrees_with_the_rust_walk() {
        let file = vectors();
        for vector in &file.vectors {
            let input = decode_hex(&vector.input)
                .unwrap_or_else(|error| panic!("{}: {error}", vector.name));
            let result = strip_raster_metadata(&input, mime_of(&vector.mime));

            if !vector.expect.ok {
                // A file this pass cannot walk is refused rather than handed
                // back: the bytes were never examined, so calling them clean
                // would be a promise nothing here can keep.
                assert!(
                    result.is_err(),
                    "{}: a file the walk cannot follow must be refused",
                    vector.name
                );
                continue;
            }

            let stripped = match result {
                Ok(stripped) => stripped,
                Err(error) => panic!("{}: expected a stripped file, got: {error}", vector.name),
            };

            // Sorted here only: the JSON carries a set, and the order the
            // composer's sentence puts them in is a separate promise, asserted
            // on the TypeScript side.
            let mut produced: Vec<String> = stripped
                .removed
                .iter()
                .map(|rule| rule.as_str().to_string())
                .collect();
            produced.sort();
            let expected_rules = vector
                .expect
                .removed
                .as_ref()
                .unwrap_or_else(|| panic!("{}: a passing vector names its rules", vector.name));
            assert_eq!(
                produced.as_slice(),
                expected_rules.as_slice(),
                "{}: rules fired",
                vector.name
            );

            let output =
                vector.expect.output.as_deref().unwrap_or_else(|| {
                    panic!("{}: a passing vector names its output", vector.name)
                });
            let expected =
                decode_hex(output).unwrap_or_else(|error| panic!("{}: {error}", vector.name));
            assert_eq!(stripped.bytes, expected, "{}: output bytes", vector.name);

            if expected_rules.is_empty() {
                // A clean vector is a file this pass may not rewrite at all, so
                // the assertion is against the input bytes.
                assert_eq!(
                    stripped.bytes, input,
                    "{}: a clean file comes back unchanged",
                    vector.name
                );
            }
        }
    }

    /// The sniff is what stops the declared type from choosing whether the rule
    /// runs, so it is pinned directly as well as through the caller.
    #[test]
    fn the_sniff_reads_the_container_from_the_leading_bytes() {
        assert_eq!(
            sniff_raster_mime(&vector_input(
                "a clean jpeg: SOI, JFIF APP0, DQT, one scan, EOI"
            )),
            Some(RasterMime::Jpeg)
        );
        assert_eq!(sniff_raster_mime(&clean_png(0x12)), Some(RasterMime::Png));
        // Neither container: this is the SVG path, and it is also what a file
        // declared a raster must not look like.
        assert_eq!(sniff_raster_mime(b"not a png at all"), None);
        assert_eq!(sniff_raster_mime(b""), None);
        // `SOI` without the `FF` of the first marker is not a JPEG. Every
        // marker after `SOI` is introduced by `FF`, so a file that stops there
        // is not one this pass could walk, and naming it would only move the
        // refusal from here to the walk.
        assert_eq!(sniff_raster_mime(&[0xff, 0xd8]), None);
        assert_eq!(sniff_raster_mime(&[0xff, 0xd8, 0x00]), None);
        // The PNG signature is eight bytes and the sniff wants all of them.
        assert_eq!(sniff_raster_mime(&PNG_SIGNATURE[..7]), None);
        assert_eq!(sniff_raster_mime(&PNG_SIGNATURE), Some(RasterMime::Png));
    }

    #[test]
    fn only_the_two_raster_containers_have_a_walk() {
        assert_eq!(
            RasterMime::from_mime_type("image/jpeg"),
            Some(RasterMime::Jpeg)
        );
        assert_eq!(
            RasterMime::from_mime_type("image/png"),
            Some(RasterMime::Png)
        );
        // SVG is outside this pass on purpose: the frontend sanitises SVG
        // source and the daemon does not. A `None` here means the caller writes
        // the bytes as they arrived, which is the gap `materialize` names.
        assert_eq!(RasterMime::from_mime_type("image/svg+xml"), None);
    }

    /// The vocabulary is a wire contract, not an internal choice: the names
    /// that leave here are the names the vectors and the composer's notice use.
    #[test]
    fn a_rule_serialises_to_the_name_the_vectors_use() {
        let names: Vec<&str> = RasterMetadataRule::ALL
            .iter()
            .map(|rule| rule.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "exif",
                "xmp",
                "iptc",
                "comment",
                "textChunk",
                "timestamp",
                "signature",
                "trailingData",
                "unknownChunk"
            ]
        );
        for rule in RasterMetadataRule::ALL {
            assert_eq!(
                serde_json::to_string(&rule).expect("a rule serialises"),
                format!("\"{}\"", rule.as_str())
            );
        }
    }

    #[test]
    fn the_test_fixture_is_a_clean_png() {
        // Pins the fixture the calling tests rely on: if the walk starts
        // removing something from it, those tests would be measuring the wrong
        // thing rather than failing outright.
        let stripped = strip_raster_metadata(&clean_png(0x01), RasterMime::Png).expect("clean");
        assert!(stripped.removed.is_empty());
        assert_eq!(stripped.bytes, clean_png(0x01));
    }
}
