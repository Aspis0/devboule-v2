//! The rules one `session_send` request's attachments must satisfy.
//!
//! These live on the wire side, next to the caps they enforce, so the daemon
//! and the app cannot drift apart. Both call [`validate_attachments`] and both
//! refuse with the string it returns, wrapped in their own error type; that is
//! the same arrangement [`crate::MAX_WRITE_BYTES`] has, one step further
//! because a rule set of seven checks is not worth writing out twice.
//!
//! The inline attachments and the references to deposited ones are validated
//! by the same file: [`validate_attachments`] for the bytes that travel in the
//! frame, [`validate_attachment_references`] for the digests that do not, and
//! [`validate_session_send_attachments`] as the one entry point both sides
//! call so neither can validate one half and forget the other.
//!
//! This is a *client* check. The daemon does not trust it: it decodes every
//! `data` itself before writing a file, and that decode is the authoritative
//! one. Passing here only means the request is well-formed enough to be worth
//! decoding.

use crate::messages::{AttachmentReference, PromptAttachment};
use crate::text_safety::{is_invisible_format, is_mandatory_line_break};
use crate::{
    MAX_ATTACHMENTS_TOTAL_BYTES, MAX_ATTACHMENT_COUNT, MAX_ATTACHMENT_DATA_BYTES,
    MAX_ATTACHMENT_NAME_BYTES, MAX_ATTACHMENT_OWNER_BYTES, MAX_ATTACHMENT_REFERENCES,
    MAX_UPLOADED_FILES, MAX_UPLOAD_BYTES, MAX_UPLOAD_CHUNK_BYTES,
};

/// The media types a prompt may carry.
///
/// Everything else is refused rather than guessed at: a daemon that writes an
/// attachment to disk and names a path in a prompt is making a promise about
/// what is in that file, and it can only keep a promise about formats it knows.
pub const ATTACHMENT_MIME_TYPES: [&str; 6] = [
    "image/png",
    "image/jpeg",
    "image/svg+xml",
    "text/markdown",
    "image/gif",
    "image/webp",
];

/// Whether a type is one a sender may carry only to a daemon that agreed
/// `attachments.gif_webp`.
///
/// The validator below admits both types on every side so the daemon can
/// answer a peer that sends one; the *sender's* refusal to send them to a
/// daemon that predates them is the client's, keyed on this.
pub fn is_gif_webp_mime(mime_type: &str) -> bool {
    matches!(mime_type, "image/gif" | "image/webp")
}

/// The display name an uploaded file is kept under, made safe to put in a
/// prompt line and on a chip.
///
/// The daemon never builds a path from this name — a stored file is named by
/// its digest — so this is not the traversal guard; it is what keeps a wire
/// name from being echoed as structure. Every separator is dropped by taking
/// the basename, control characters, mandatory line breaks (U+2028, U+2029,
/// U+0085) and invisible formatting (bidi overrides, zero-width marks) become
/// `_` — one of those could forge a prompt line or make the chip read as a
/// different name — and the Windows-invalid set becomes `_` too, a reserved
/// device name is prefixed with `_` (a chip reading `CON` is a name nobody can
/// act on), trailing dots and spaces are trimmed (Win32 would trim them anyway,
/// so the name would not round-trip), and the result is cut to
/// [`MAX_ATTACHMENT_NAME_BYTES`] bytes on a character boundary. Unicode is
/// kept.
pub fn sanitize_attachment_name(name: &str) -> String {
    let basename = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut cleaned = String::with_capacity(basename.len());
    for character in basename.trim_matches(' ').chars() {
        if character.is_control()
            || is_mandatory_line_break(character)
            || is_invisible_format(character)
            || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*')
        {
            cleaned.push('_');
        } else {
            cleaned.push(character);
        }
    }
    let trimmed = cleaned.trim_end_matches([' ', '.']);
    let stem = trimmed.split('.').next().unwrap_or(trimmed);
    let mut safe = if is_reserved_device_name(stem) {
        format!("_{trimmed}")
    } else {
        trimmed.to_string()
    };
    if safe.is_empty() {
        safe.push_str("file");
    }
    while safe.len() > MAX_ATTACHMENT_NAME_BYTES {
        let mut cut = MAX_ATTACHMENT_NAME_BYTES;
        while !safe.is_char_boundary(cut) {
            cut -= 1;
        }
        safe.truncate(cut);
    }
    safe
}

/// The Windows device names a plain namespace claims, whichever case and
/// whichever extension follows them (`CON`, `con.txt`, `NUL.log`).
fn is_reserved_device_name(stem: &str) -> bool {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    RESERVED
        .iter()
        .any(|device| stem.eq_ignore_ascii_case(device))
}

/// The extension a stored file is written under, from its display name:
/// ASCII alphanumeric, at most eight characters, lowercased. Anything else is
/// `None` and the store writes `bin`.
///
/// The extension is a name the store joins to a digest, so the alphabet is the
/// point: a name cannot widen the search beyond one directory entry.
pub fn upload_extension(name: &str) -> Option<String> {
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if extension.is_empty()
        || extension.len() > 8
        || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(extension)
}

/// Whether `upload_id` is a token an in-progress upload may be keyed by:
/// 1..=64 lowercase ASCII alphanumerics, `-` or `_`.
///
/// The id is the client's, because a reconnect has to name the same upload to
/// resume it; the alphabet is what keeps an id from being a path or an empty
/// key. Uppercase is refused rather than folded: the id names a staged file on
/// a case-preserving but case-insensitive filesystem (NTFS, APFS), so `Up-1`
/// and `up-1` would be two map entries over one file.
pub fn validate_upload_id(upload_id: &str) -> Result<(), String> {
    let shape = !upload_id.is_empty()
        && upload_id.len() <= 64
        && upload_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        });
    if shape {
        return Ok(());
    }
    Err("An upload id must be 1 to 64 lowercase letters, digits, '-' or '_'.".to_string())
}

/// The first reason an upload may not be opened as declared, or `Ok(())`.
pub fn validate_upload_begin(total_bytes: u64) -> Result<(), String> {
    if total_bytes == 0 {
        return Err("An uploaded file is empty.".to_string());
    }
    if total_bytes > MAX_UPLOAD_BYTES {
        return Err(format!(
            "An uploaded file is {total_bytes} bytes; the limit is {MAX_UPLOAD_BYTES}."
        ));
    }
    Ok(())
}

/// The first reason a chunk may not be appended to an upload at `received`, or
/// `Ok(())`.
///
/// The offset rule is strict append: a chunk that does not start exactly where
/// the upload stands is refused, and the refusal names both numbers so a client
/// that lost an acknowledgement can ask [`crate::messages::ClientMessage::SessionUploadStatus`]
/// and continue — or abort and start again.
pub fn validate_upload_chunk(
    data: &str,
    offset: u64,
    received: u64,
    total_bytes: u64,
) -> Result<(), String> {
    if !is_valid_base64(data) {
        return Err(invalid_base64_message());
    }
    let padding = data.bytes().rev().take_while(|byte| *byte == b'=').count();
    let raw_len = ((data.len() / 4) * 3).saturating_sub(padding);
    if raw_len > MAX_UPLOAD_CHUNK_BYTES {
        return Err(format!(
            "An upload chunk is {raw_len} bytes; the limit is {MAX_UPLOAD_CHUNK_BYTES}."
        ));
    }
    if offset != received {
        return Err(format!(
            "The upload is at byte {received}; the chunk starts at {offset}."
        ));
    }
    if received.saturating_add(raw_len as u64) > total_bytes {
        return Err(format!(
            "The chunk would take the upload past its declared {total_bytes} bytes."
        ));
    }
    Ok(())
}

/// The number of hex characters a SHA-256 digest has (32 bytes).
///
/// Not a cap to round: it is the one length the store's `sha256_hex` emits, and
/// a digest that is not this long did not come from a deposit.
const DIGEST_HEX_LEN: usize = 64;

/// The rejection for a `data` that is not base64.
pub fn invalid_base64_message() -> String {
    "An attachment's data is not valid base64.".to_string()
}

/// The rejection for a `data` that is empty.
///
/// The empty string is well-formed base64 — it encodes zero bytes — so this is
/// a rule about attachments, not about the encoding. A zero-byte file is not an
/// image, and handing the agent a path to one is a promise the daemon cannot
/// keep.
pub fn empty_attachment_message() -> String {
    "An attachment's data is empty.".to_string()
}

/// The rejection for a `mime_type` outside [`ATTACHMENT_MIME_TYPES`].
pub fn unsupported_attachment_type_message(mime_type: &str) -> String {
    format!(
        "Attachment type '{}' is not supported; expected one of {}.",
        excerpt(mime_type),
        ATTACHMENT_MIME_TYPES.join(", ")
    )
}

/// The rejection for a `name` longer than [`MAX_ATTACHMENT_NAME_BYTES`].
///
/// The byte count travels; the name does not, because the name is what may be
/// arbitrarily long.
pub fn attachment_name_too_long_message(name: &str) -> String {
    format!(
        "An attachment's name is {} bytes; the limit is {MAX_ATTACHMENT_NAME_BYTES}.",
        name.len()
    )
}

/// At most 64 bytes of an untrusted string, cut on a character boundary.
///
/// Every string this module puts into a rejection arrives from the wire, and
/// two of them — a file's `name` and its `mime_type` — have no length of their
/// own that anything else bounds. Echoing one whole would move the flood the
/// caps exist to prevent out of the frame and into the error string, which the
/// app then renders. The cut lands on a character boundary, so what comes back
/// is always valid UTF-8, and the ellipsis says the value was longer rather
/// than letting a truncated one pass for the whole.
fn excerpt(value: &str) -> String {
    const EXCERPT_BYTES: usize = 64;
    if value.len() <= EXCERPT_BYTES {
        return value.to_string();
    }
    let mut end = EXCERPT_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

/// The rejection for a digest that is not a SHA-256 hex string.
pub fn invalid_attachment_digest_message() -> String {
    format!("An attachment reference's digest is not {DIGEST_HEX_LEN} lowercase hex characters.")
}

/// The rejection for a reference to a session other than the request's.
///
/// Both session ids go through [`excerpt`]: each is untrusted wire input, and
/// echoing either whole would put the flood back into a string the app renders.
pub fn attachment_reference_session_mismatch_message(
    reference_session: &str,
    request_session: &str,
) -> String {
    format!(
        "An attachment reference belongs to session '{}', not '{}'.",
        excerpt(reference_session),
        excerpt(request_session)
    )
}

/// Which attachment a rejection is about: its 1-based position and its name.
///
/// The name goes through [`excerpt`], which is why a rejection about a file
/// with an enormous name is still a sentence. An empty name is legal and yields
/// the position alone: a missing label is not a reason to refuse a file, and the
/// position alone already answers "which of the four".
fn attachment_label(index: usize, name: &str) -> String {
    let position = index + 1;
    if name.is_empty() {
        return format!("Attachment {position}: ");
    }
    format!("Attachment {position} ('{}'): ", excerpt(name))
}

/// Which reference a rejection is about: its 1-based position and its digest.
///
/// Like [`attachment_label`], but there is no name on a reference and the
/// digest is what a person can act on. It goes through [`excerpt`], so a
/// malformed megabyte-long digest is still a sentence.
fn reference_label(index: usize, digest: &str) -> String {
    format!(
        "Attachment reference {} ('{}'): ",
        index + 1,
        excerpt(digest)
    )
}

/// The first reason these attachments cannot be sent, or `Ok(())`.
///
/// The order of the checks is the order a caller should fix them in: too many
/// files, then a file's type, then whether its data is empty, then its size,
/// then its encoding, then its name, then the total. Every per-file rejection is
/// prefixed with [`attachment_label`], so a message about one of four
/// attachments says which one. The count and total rejections are about the
/// request rather than one file, so they keep their unlabelled wording.
pub fn validate_attachments(attachments: &[PromptAttachment]) -> Result<(), String> {
    if attachments.len() > MAX_ATTACHMENT_COUNT {
        return Err(format!(
            "A prompt may carry at most {MAX_ATTACHMENT_COUNT} attachments; {} were sent.",
            attachments.len()
        ));
    }
    let mut total = 0;
    for (index, attachment) in attachments.iter().enumerate() {
        let label = attachment_label(index, &attachment.name);
        if !ATTACHMENT_MIME_TYPES.contains(&attachment.mime_type.as_str()) {
            return Err(format!(
                "{label}{}",
                unsupported_attachment_type_message(&attachment.mime_type)
            ));
        }
        if attachment.data.is_empty() {
            return Err(format!("{label}{}", empty_attachment_message()));
        }
        if attachment.data.len() > MAX_ATTACHMENT_DATA_BYTES {
            return Err(format!(
                "{label}One attachment's data is {} bytes of base64; the limit is {MAX_ATTACHMENT_DATA_BYTES}.",
                attachment.data.len()
            ));
        }
        if !is_valid_base64(&attachment.data) {
            return Err(format!("{label}{}", invalid_base64_message()));
        }
        if attachment.name.len() > MAX_ATTACHMENT_NAME_BYTES {
            return Err(format!(
                "{label}{}",
                attachment_name_too_long_message(&attachment.name)
            ));
        }
        // Cannot overflow: the loop is bounded by MAX_ATTACHMENT_COUNT and each
        // item by MAX_ATTACHMENT_DATA_BYTES, both far under usize::MAX.
        total += attachment.data.len();
    }
    if total > MAX_ATTACHMENTS_TOTAL_BYTES {
        return Err(format!(
            "The attachments add up to {total} bytes of base64; the limit is {MAX_ATTACHMENTS_TOTAL_BYTES}."
        ));
    }
    Ok(())
}

/// The first reason these stored-attachment references cannot be sent, or
/// `Ok(())`.
///
/// The order matches [`validate_attachments`]: the count first, then each
/// reference, then the total. Two rules here are not about one reference's
/// shape:
///
/// - A reference must name **this** session. A digest resolves only inside the
///   session it was deposited to, so a reference to another session is refused
///   before the digest is looked at; the store enforces the same rule against
///   its directory layout, and this is the wire half that keeps a digest from
///   ever arriving with no session attached to it.
/// - The references' stored bytes are summed against
///   [`MAX_ATTACHMENT_OWNER_BYTES`], the cumulative budget for one owner. A
///   single prompt can only be refused here when it alone would exceed what the
///   owner may store; the daemon's directory walk is what enforces the total
///   across prompts. `stored_bytes` is advisory for that reason and is never
///   the number the daemon's own budget trusts.
///
/// An empty list is `Ok(())`: a prompt that carries only inline attachments, or
/// no attachments at all, is the common case and not a mistake.
pub fn validate_attachment_references(
    session_id: &str,
    references: &[AttachmentReference],
) -> Result<(), String> {
    if references.len() > MAX_ATTACHMENT_REFERENCES {
        return Err(format!(
            "A prompt may refer to at most {MAX_ATTACHMENT_REFERENCES} stored attachments; {} were sent.",
            references.len()
        ));
    }
    let uploaded = references
        .iter()
        .filter(|reference| !reference.name.is_empty())
        .count();
    if uploaded > MAX_UPLOADED_FILES {
        return Err(format!(
            "A prompt may carry at most {MAX_UPLOADED_FILES} uploaded files; {uploaded} were sent."
        ));
    }
    let mut total: u64 = 0;
    for (index, reference) in references.iter().enumerate() {
        let label = reference_label(index, &reference.digest);
        if reference.session_id != session_id {
            return Err(format!(
                "{label}{}",
                attachment_reference_session_mismatch_message(&reference.session_id, session_id)
            ));
        }
        if !is_valid_digest(&reference.digest) {
            return Err(format!("{label}{}", invalid_attachment_digest_message()));
        }
        // Saturating, because the sum is attacker-controlled and a wrapping
        // add could land back under the limit. The loop is bounded by
        // MAX_ATTACHMENT_REFERENCES, so the true sum never approaches u64::MAX
        // anyway; saturation is for the malformed case, not the real one.
        total = total.saturating_add(reference.stored_bytes);
    }
    if total > MAX_ATTACHMENT_OWNER_BYTES as u64 {
        return Err(format!(
            "The stored attachments this prompt refers to add up to {total} bytes; the limit is {MAX_ATTACHMENT_OWNER_BYTES}."
        ));
    }
    Ok(())
}

/// Both halves of one `session_send`'s attachments: the inline files and the
/// references to stored ones.
///
/// The single entry point for the app and the daemon. Inline first, because
/// those bytes are in the frame the caller already built; the references are
/// validated against `session_id`, which the caller must pass rather than let
/// the references imply.
pub fn validate_session_send_attachments(
    session_id: &str,
    attachments: &[PromptAttachment],
    references: &[AttachmentReference],
) -> Result<(), String> {
    validate_attachments(attachments)?;
    validate_attachment_references(session_id, references)
}

/// Whether `digest` is a SHA-256 digest as the store writes it: 64 lowercase
/// hex characters.
///
/// Uppercase is refused as well as non-hex. `sha256_hex` emits lowercase, the
/// deposit reply hands back exactly that, and a client is meant to use the
/// value it was given rather than re-case it; accepting both spellings would
/// make two strings for one file and leave the store to decide which one keys
/// the lookup.
fn is_valid_digest(digest: &str) -> bool {
    digest.len() == DIGEST_HEX_LEN
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether `data` is well-formed base64 in the standard alphabet (RFC 4648 §4).
///
/// Checks the shape only — alphabet, length, padding placement — which is what
/// the frame needs. Whether the trailing bits are zero, and therefore whether
/// the encoding is canonical, is the decoder's business; `materialize` runs the
/// real decoder and refuses with [`invalid_base64_message`] if it disagrees.
///
/// The empty string is valid: it is the encoding of zero bytes.
fn is_valid_base64(data: &str) -> bool {
    let bytes = data.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return false;
    }
    let padding = bytes.iter().rev().take_while(|byte| **byte == b'=').count();
    if padding > 2 {
        return false;
    }
    bytes[..bytes.len() - padding]
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'+' || *byte == b'/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(mime_type: &str, data: &str) -> PromptAttachment {
        PromptAttachment {
            name: "a.png".to_string(),
            mime_type: mime_type.to_string(),
            data: data.to_string(),
        }
    }

    #[test]
    fn an_enormous_mime_type_is_not_echoed_whole() {
        // `mime_type` is the last field on the wire that no cap bounds: the
        // allowlist refuses an unknown one rather than a length rule, so a frame
        // can carry a huge one and be rejected for its type. The rejection must
        // not carry it back — that would move the flood out of the frame and
        // into a string the app renders.
        let shouting = "image/".to_string() + &"z".repeat(200_000);
        let message = unsupported_attachment_type_message(&shouting);
        assert!(
            message.len() < 200,
            "the rejection is a sentence: {}",
            message.len()
        );
        assert!(!message.contains(&shouting));
        assert!(message.contains("image/zzz"));
        assert!(message.contains('…'));
    }

    #[test]
    fn an_excerpt_never_splits_a_character() {
        // A cut at a fixed byte count lands inside a multi-byte character unless
        // it walks back to a boundary; the result would not be valid UTF-8 and
        // the format! would panic rather than return a message.
        for repeat in 1..40 {
            let value = "è".repeat(repeat);
            let cut = excerpt(&value);
            assert!(cut.chars().count() > 0);
            assert!(value.starts_with(cut.trim_end_matches('…')));
        }
    }

    #[test]
    fn base64_shape_check_matches_the_alphabet_and_padding_rules() {
        for valid in ["", "AA==", "AAA=", "AAAA", "QUJD", "+/+/"] {
            assert!(is_valid_base64(valid), "{valid} should be valid");
        }
        for invalid in [
            "A", "AAA", "AAAAA", "A===", "=AAA", "AB=C", "A B=", "AA=A", "AA==\n",
        ] {
            assert!(!is_valid_base64(invalid), "{invalid} should be invalid");
        }
    }

    #[test]
    fn the_count_limit_is_the_first_thing_reported() {
        let five = vec![attachment("image/png", "AA=="); MAX_ATTACHMENT_COUNT + 1];
        let message = validate_attachments(&five).expect_err("rejected");
        assert!(
            message.contains(&MAX_ATTACHMENT_COUNT.to_string()),
            "{message}"
        );
        assert!(message.contains('5'), "{message}");
    }

    #[test]
    fn an_unsupported_type_names_the_type_and_the_allowed_set() {
        let message =
            validate_attachments(&[attachment("image/bmp", "AA==")]).expect_err("rejected");
        assert!(message.starts_with("Attachment 1 ('a.png'): "), "{message}");
        assert!(message.contains("image/bmp"), "{message}");
        for allowed in ATTACHMENT_MIME_TYPES {
            assert!(message.contains(allowed), "{message} should name {allowed}");
        }
    }

    #[test]
    fn gif_and_webp_are_attachment_types() {
        validate_attachments(&[
            attachment("image/gif", "AA=="),
            attachment("image/webp", "AA=="),
        ])
        .expect("both animated-capable rasters are carried");
        assert!(is_gif_webp_mime("image/gif"));
        assert!(is_gif_webp_mime("image/webp"));
        for other in ["image/png", "image/jpeg", "image/svg+xml", "text/markdown"] {
            assert!(!is_gif_webp_mime(other), "{other} rides no capability");
        }
    }

    #[test]
    fn an_oversized_attachment_names_the_limit() {
        let oversized = "A".repeat(MAX_ATTACHMENT_DATA_BYTES + 4);
        let message =
            validate_attachments(&[attachment("image/png", &oversized)]).expect_err("rejected");
        assert!(
            message.contains(&MAX_ATTACHMENT_DATA_BYTES.to_string()),
            "{message}"
        );
    }

    #[test]
    fn an_unencoded_attachment_is_refused() {
        let message =
            validate_attachments(&[attachment("image/png", "not base64!")]).expect_err("rejected");
        assert_eq!(
            message,
            format!("Attachment 1 ('a.png'): {}", invalid_base64_message())
        );
    }

    #[test]
    fn an_attachment_with_no_bytes_is_refused() {
        let message = validate_attachments(&[attachment("image/png", "")]).expect_err("rejected");
        assert_eq!(
            message,
            format!("Attachment 1 ('a.png'): {}", empty_attachment_message())
        );
        // The refusal is a rule about attachments, not about base64: the empty
        // string stays well-formed.
        assert!(is_valid_base64(""));
    }

    #[test]
    fn a_rejection_names_the_attachment_it_is_about() {
        let four = vec![
            attachment("image/png", "AA=="),
            attachment("image/png", "AA=="),
            attachment("image/bmp", "AA=="),
            attachment("image/png", "AA=="),
        ];
        let message = validate_attachments(&four).expect_err("rejected");
        assert!(
            message.starts_with("Attachment 3 ('a.png'): "),
            "four attachments must not produce an ambiguous message: {message}"
        );
    }

    #[test]
    fn a_long_name_is_refused_and_its_label_is_truncated() {
        // Two bytes per character, so 255 characters are 510 bytes: over the
        // cap, and the 64-byte cut would land mid-character without the
        // boundary walk.
        let long = "\u{e9}".repeat(MAX_ATTACHMENT_NAME_BYTES);
        let mut item = attachment("image/png", "AA==");
        item.name = long.clone();

        let message = validate_attachments(&[item]).expect_err("rejected");
        assert!(
            message.contains(&attachment_name_too_long_message(&long)),
            "{message}"
        );
        assert!(
            message.starts_with(&format!(
                "Attachment 1 ('{}\u{2026}'): ",
                "\u{e9}".repeat(32)
            )),
            "{message}"
        );
        assert!(
            !message.contains(&long),
            "the whole name must not be echoed: {message}"
        );
    }

    #[test]
    fn an_empty_name_is_labelled_by_position_alone() {
        let mut item = attachment("image/bmp", "AA==");
        item.name = String::new();
        let message = validate_attachments(&[item]).expect_err("rejected");
        assert!(message.starts_with("Attachment 1: "), "{message}");
        assert!(message.contains("image/bmp"), "{message}");
    }

    #[test]
    fn a_total_over_the_limit_names_the_limit() {
        // Four items each just under the per-item cap: only the total is wrong.
        let each = "A".repeat(MAX_ATTACHMENT_DATA_BYTES - 4);
        let four = vec![attachment("image/png", &each); MAX_ATTACHMENT_COUNT];
        assert!(
            each.len() * MAX_ATTACHMENT_COUNT > MAX_ATTACHMENTS_TOTAL_BYTES,
            "the fixture must exceed the total and not the per-item cap"
        );
        let message = validate_attachments(&four).expect_err("rejected");
        assert!(
            message.contains(&MAX_ATTACHMENTS_TOTAL_BYTES.to_string()),
            "{message}"
        );
    }

    #[test]
    fn a_request_at_every_limit_passes() {
        let each = "A".repeat(MAX_ATTACHMENTS_TOTAL_BYTES / MAX_ATTACHMENT_COUNT);
        let four = vec![attachment("image/jpeg", &each); MAX_ATTACHMENT_COUNT];
        validate_attachments(&four).expect("at every cap");
    }

    /// A digest shaped the way `sha256_hex` writes one: 64 lowercase hex
    /// characters. `seed` must itself be a lowercase hex character.
    fn digest_of(seed: char) -> String {
        seed.to_string().repeat(DIGEST_HEX_LEN)
    }

    fn reference(session_id: &str, digest: &str) -> AttachmentReference {
        AttachmentReference {
            session_id: session_id.to_string(),
            digest: digest.to_string(),
            stored_bytes: 1024,
            name: String::new(),
        }
    }

    #[test]
    fn a_name_is_a_basename_and_never_structure() {
        assert_eq!(sanitize_attachment_name("report.pdf"), "report.pdf");
        assert_eq!(sanitize_attachment_name("dir/report.pdf"), "report.pdf");
        assert_eq!(
            sanitize_attachment_name("dir\\sub\\report.pdf"),
            "report.pdf"
        );
        assert_eq!(sanitize_attachment_name(".."), "file");
        assert_eq!(sanitize_attachment_name("../../etc/passwd"), "passwd");
        assert_eq!(
            sanitize_attachment_name("C:\\Windows\\evil.txt"),
            "evil.txt"
        );
        assert_eq!(
            sanitize_attachment_name("\"quote\"|pipe.txt"),
            "_quote__pipe.txt"
        );
        assert_eq!(sanitize_attachment_name("a\nb.txt"), "a_b.txt");
        assert_eq!(sanitize_attachment_name("nul"), "_nul");
        assert_eq!(sanitize_attachment_name("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_attachment_name("lpt3.log"), "_lpt3.log");
        assert_eq!(sanitize_attachment_name("trailing."), "trailing");
        assert_eq!(sanitize_attachment_name("trailing "), "trailing");
        assert_eq!(sanitize_attachment_name("  "), "file");
        assert_eq!(sanitize_attachment_name("rapport-é.pdf"), "rapport-é.pdf");
    }

    #[test]
    fn a_line_break_or_override_that_is_not_ascii_control_is_replaced() {
        // U+2028/U+2029/U+0085 render as newlines without being `char::is_control`,
        // and a bidi override makes the chip read as a different name.
        assert_eq!(
            sanitize_attachment_name("x\u{2028}Uploaded file: y.pdf"),
            "x_Uploaded file_ y.pdf"
        );
        assert_eq!(sanitize_attachment_name("x\u{2029}Path: z"), "x_Path_ z");
        assert_eq!(sanitize_attachment_name("x\u{85}y.pdf"), "x_y.pdf");
        assert_eq!(
            sanitize_attachment_name("invoice\u{202e}gpj.exe"),
            "invoice_gpj.exe"
        );
        assert_eq!(sanitize_attachment_name("a\u{200b}b.pdf"), "a_b.pdf");
    }

    #[test]
    fn a_long_name_is_cut_on_a_character_boundary() {
        let name = format!("{}.pdf", "é".repeat(400));
        let safe = sanitize_attachment_name(&name);
        assert!(safe.len() <= MAX_ATTACHMENT_NAME_BYTES, "{}", safe.len());
        assert!(safe.len() > MAX_ATTACHMENT_NAME_BYTES - 4, "{}", safe.len());
        assert!(std::str::from_utf8(safe.as_bytes()).is_ok());
    }

    #[test]
    fn the_extension_is_a_small_ascii_alphabet() {
        assert_eq!(upload_extension("report.PDF"), Some("pdf".to_string()));
        assert_eq!(upload_extension("archive.tar.gz"), Some("gz".to_string()));
        assert_eq!(upload_extension("no-extension"), None);
        assert_eq!(upload_extension("bad.ext two"), None);
        assert_eq!(upload_extension("too.longextension"), None);
        assert_eq!(upload_extension("dot."), None);
    }

    #[test]
    fn an_upload_id_is_a_token_not_a_path() {
        assert!(validate_upload_id("0f7c-2a_9").is_ok());
        for bad in [
            "",
            "../up",
            "a/b",
            "a\\b",
            "a b",
            "Up-1",
            "x".repeat(65).as_str(),
        ] {
            assert!(validate_upload_id(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn an_upload_is_bounded_before_its_first_byte() {
        assert!(validate_upload_begin(1).is_ok());
        assert!(validate_upload_begin(MAX_UPLOAD_BYTES).is_ok());
        assert!(validate_upload_begin(0)
            .expect_err("empty")
            .contains("empty"));
        assert!(validate_upload_begin(MAX_UPLOAD_BYTES + 1)
            .expect_err("oversize")
            .contains(&MAX_UPLOAD_BYTES.to_string()));
    }

    #[test]
    fn a_chunk_must_start_where_the_upload_stands() {
        // "AAAA" is three raw bytes.
        assert!(validate_upload_chunk("AAAA", 0, 0, 10).is_ok());
        assert!(validate_upload_chunk("AAAA", 3, 3, 10).is_ok());
        let stale = validate_upload_chunk("AAAA", 0, 3, 10).expect_err("stale offset");
        assert!(stale.contains('3') && stale.contains('0'), "{stale}");
        let past_end = validate_upload_chunk("AAAA", 8, 8, 10).expect_err("past the end");
        assert!(past_end.contains("10"), "{past_end}");
        assert!(validate_upload_chunk("not base64", 0, 0, 10).is_err());
        assert!(validate_upload_chunk("====", 0, 0, 10).is_err());
        let oversized = "A".repeat(MAX_UPLOAD_CHUNK_BYTES / 3 * 4 + 8);
        assert!(validate_upload_chunk(&oversized, 0, 0, MAX_UPLOAD_BYTES)
            .expect_err("chunk cap")
            .contains(&MAX_UPLOAD_CHUNK_BYTES.to_string()));
    }

    #[test]
    fn a_prompt_may_carry_at_most_eight_uploaded_files() {
        let mut files = vec![reference("s.a.1", &digest_of('a')); MAX_UPLOADED_FILES];
        for file in &mut files {
            file.name = "report.pdf".to_string();
        }
        validate_attachment_references("s.a.1", &files).expect("eight files fit");
        files.push(reference("s.a.1", &digest_of('b')));
        files.last_mut().expect("one more").name = "ninth.log".to_string();
        let message = validate_attachment_references("s.a.1", &files).expect_err("ninth");
        assert!(
            message.contains(&MAX_UPLOADED_FILES.to_string()),
            "{message}"
        );
    }

    #[test]
    fn a_prompt_may_name_at_most_the_renderer_page_ceiling() {
        let too_many = vec![reference("s.a.1", &digest_of('a')); MAX_ATTACHMENT_REFERENCES + 1];
        let message = validate_attachment_references("s.a.1", &too_many).expect_err("rejected");
        assert!(
            message.contains(&MAX_ATTACHMENT_REFERENCES.to_string()),
            "{message}"
        );
        assert!(
            message.contains(&(MAX_ATTACHMENT_REFERENCES + 1).to_string()),
            "{message}"
        );
    }

    #[test]
    fn an_empty_reference_list_is_fine() {
        // The common request: inline attachments only, or none at all. The
        // reference rules must not turn "no references" into a refusal, and
        // the combined entry point has to keep accepting a send that predates
        // the deposit call entirely.
        validate_attachment_references("s.a.1", &[]).expect("no references is not an error");
        validate_session_send_attachments("s.a.1", &[], &[]).expect("no attachments at all");
    }

    #[test]
    fn a_reference_over_the_owner_byte_budget_is_refused() {
        let mut item = reference("s.a.1", &digest_of('b'));
        item.stored_bytes = MAX_ATTACHMENT_OWNER_BYTES as u64 + 1;
        let message = validate_attachment_references("s.a.1", &[item]).expect_err("rejected");
        assert!(
            message.contains(&MAX_ATTACHMENT_OWNER_BYTES.to_string()),
            "{message}"
        );
        assert!(
            message.contains(&(MAX_ATTACHMENT_OWNER_BYTES as u64 + 1).to_string()),
            "{message}"
        );
    }

    #[test]
    fn references_at_the_owner_byte_budget_pass_and_one_byte_over_does_not() {
        // Two halves, so the rule is the *sum* and not a single value that
        // happens to sit near the bound.
        let half = (MAX_ATTACHMENT_OWNER_BYTES / 2) as u64;
        let mut first = reference("s.a.1", &digest_of('c'));
        let mut second = reference("s.a.1", &digest_of('d'));
        first.stored_bytes = half;
        second.stored_bytes = MAX_ATTACHMENT_OWNER_BYTES as u64 - half;
        validate_attachment_references("s.a.1", &[first.clone(), second.clone()])
            .expect("exactly the budget is allowed");
        second.stored_bytes += 1;
        let message =
            validate_attachment_references("s.a.1", &[first, second]).expect_err("one over");
        assert!(
            message.contains(&MAX_ATTACHMENT_OWNER_BYTES.to_string()),
            "{message}"
        );
    }

    #[test]
    fn a_malformed_digest_is_refused_and_the_valid_shape_is_defined() {
        let owned = [
            // Uppercase is the same digest with a second spelling. Refusing it
            // keeps one string per stored file.
            "A".repeat(DIGEST_HEX_LEN),
            "g".repeat(DIGEST_HEX_LEN),
            "a".repeat(DIGEST_HEX_LEN - 1),
            "a".repeat(DIGEST_HEX_LEN + 1),
        ];
        let mut malformed: Vec<&str> = vec!["not a digest", ""];
        malformed.extend(owned.iter().map(String::as_str));
        for bad in malformed {
            let message = validate_attachment_references("s.a.1", &[reference("s.a.1", bad)])
                .expect_err("rejected");
            assert_eq!(
                message,
                format!(
                    "Attachment reference 1 ('{}'): {}",
                    excerpt(bad),
                    invalid_attachment_digest_message()
                )
            );
        }
        // Every character a `sha256_hex` digest may contain, 64 of them.
        let every_hex = "0123456789abcdef".repeat(4);
        validate_attachment_references("s.a.1", &[reference("s.a.1", &every_hex)])
            .expect("64 lowercase hex characters");
    }

    #[test]
    fn a_reference_from_another_session_is_refused_before_the_digest_is_looked_at() {
        // Deposited in `s.a.1`, presented for `s.a.2`. The session rule runs
        // first, so the refusal is about the session even when the digest is
        // also malformed; a digest resolves only inside its own session.
        let item = reference("s.a.1", "not a digest");
        let message = validate_attachment_references("s.a.2", &[item]).expect_err("rejected");
        assert_eq!(
            message,
            format!(
                "Attachment reference 1 ('not a digest'): {}",
                attachment_reference_session_mismatch_message("s.a.1", "s.a.2")
            )
        );
        assert!(message.contains("s.a.1"), "{message}");
        assert!(message.contains("s.a.2"), "{message}");
    }

    #[test]
    fn an_enormous_digest_is_not_echoed_whole() {
        let shouting = "z".repeat(200_000);
        let message = validate_attachment_references("s.a.1", &[reference("s.a.1", &shouting)])
            .expect_err("rejected");
        assert!(
            message.len() < 200,
            "the rejection is a sentence: {}",
            message.len()
        );
        assert!(!message.contains(&shouting));
        assert!(message.contains('…'));
    }

    #[test]
    fn a_reference_rejection_names_the_reference_it_is_about() {
        let refs = vec![
            reference("s.a.1", &digest_of('9')),
            reference("s.a.1", "not a digest"),
            reference("s.a.1", &digest_of('8')),
        ];
        let message = validate_attachment_references("s.a.1", &refs).expect_err("rejected");
        assert!(
            message.starts_with("Attachment reference 2 ('"),
            "two good references must not make the message ambiguous: {message}"
        );
    }

    #[test]
    fn references_and_inline_attachments_are_both_validated() {
        let good_inline = attachment("image/png", "AA==");
        let good_reference = reference("s.a.1", &digest_of('f'));
        validate_session_send_attachments(
            "s.a.1",
            std::slice::from_ref(&good_inline),
            std::slice::from_ref(&good_reference),
        )
        .expect("one inline image and one stored reference together");

        // A broken inline attachment is reported first: those bytes are in the
        // frame the caller already built, and the reference half must not mask
        // the inline half.
        let bad_inline = attachment("image/bmp", "AA==");
        let other_session = reference("s.a.2", &digest_of('f'));
        let message = validate_session_send_attachments(
            "s.a.1",
            std::slice::from_ref(&bad_inline),
            std::slice::from_ref(&other_session),
        )
        .expect_err("rejected");
        assert!(message.starts_with("Attachment 1 ('a.png'): "), "{message}");
        assert!(message.contains("image/bmp"), "{message}");

        // With the inline half valid, the reference half is what refuses; an
        // inline attachment does not turn the reference rules off.
        let message = validate_session_send_attachments(
            "s.a.1",
            std::slice::from_ref(&good_inline),
            std::slice::from_ref(&other_session),
        )
        .expect_err("rejected");
        assert!(message.starts_with("Attachment reference 1"), "{message}");
        assert!(message.contains("s.a.2"), "{message}");
    }
}
