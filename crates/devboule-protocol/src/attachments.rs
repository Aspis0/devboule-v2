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
use crate::{
    MAX_ATTACHMENTS_TOTAL_BYTES, MAX_ATTACHMENT_COUNT, MAX_ATTACHMENT_DATA_BYTES,
    MAX_ATTACHMENT_NAME_BYTES, MAX_ATTACHMENT_OWNER_BYTES, MAX_ATTACHMENT_REFERENCES,
};

/// The media types a prompt may carry.
///
/// Everything else is refused rather than guessed at: a daemon that writes an
/// attachment to disk and names a path in a prompt is making a promise about
/// what is in that file, and it can only keep a promise about formats it knows.
pub const ATTACHMENT_MIME_TYPES: [&str; 3] = ["image/png", "image/jpeg", "image/svg+xml"];

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
        }
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
        let bad_inline = attachment("image/gif", "AA==");
        let other_session = reference("s.a.2", &digest_of('f'));
        let message = validate_session_send_attachments(
            "s.a.1",
            std::slice::from_ref(&bad_inline),
            std::slice::from_ref(&other_session),
        )
        .expect_err("rejected");
        assert!(message.starts_with("Attachment 1 ('a.png'): "), "{message}");
        assert!(message.contains("image/gif"), "{message}");

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
