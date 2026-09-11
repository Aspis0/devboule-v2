//! The rules one `session_send` request's attachments must satisfy.
//!
//! These live on the wire side, next to the caps they enforce, so the daemon
//! and the app cannot drift apart. Both call [`validate_attachments`] and both
//! refuse with the string it returns, wrapped in their own error type; that is
//! the same arrangement [`crate::MAX_WRITE_BYTES`] has, one step further
//! because a rule set of seven checks is not worth writing out twice.
//!
//! This is a *client* check. The daemon does not trust it: it decodes every
//! `data` itself before writing a file, and that decode is the authoritative
//! one. Passing here only means the request is well-formed enough to be worth
//! decoding.

use crate::messages::PromptAttachment;
use crate::{
    MAX_ATTACHMENTS_TOTAL_BYTES, MAX_ATTACHMENT_COUNT, MAX_ATTACHMENT_DATA_BYTES,
    MAX_ATTACHMENT_NAME_BYTES,
};

/// The media types a prompt may carry.
///
/// Everything else is refused rather than guessed at: a daemon that writes an
/// attachment to disk and names a path in a prompt is making a promise about
/// what is in that file, and it can only keep a promise about formats it knows.
pub const ATTACHMENT_MIME_TYPES: [&str; 3] = ["image/png", "image/jpeg", "image/svg+xml"];

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
            validate_attachments(&[attachment("image/gif", "AA==")]).expect_err("rejected");
        assert!(message.starts_with("Attachment 1 ('a.png'): "), "{message}");
        assert!(message.contains("image/gif"), "{message}");
        for allowed in ATTACHMENT_MIME_TYPES {
            assert!(message.contains(allowed), "{message} should name {allowed}");
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
            attachment("image/gif", "AA=="),
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
        let mut item = attachment("image/gif", "AA==");
        item.name = String::new();
        let message = validate_attachments(&[item]).expect_err("rejected");
        assert!(message.starts_with("Attachment 1: "), "{message}");
        assert!(message.contains("image/gif"), "{message}");
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
}
