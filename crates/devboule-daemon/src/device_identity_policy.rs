//! Display-name and redaction policy: the name this daemon accepts and
//! publishes as a device name, and the non-reversible label that stands in
//! for identities, keys, pairing codes and bearers in log lines.

use sha2::Digest;

use super::{hex, MAX_DISPLAY_NAME_CHARS};

/// A short, non-reversible label for a log line. Identities, keys, pairing
/// codes and bearers never reach `eprintln!` as themselves.
///
/// Used by the hello-mismatch diagnostic in `server.rs`, which otherwise
/// carries the user SID and the `peer_<device_id>` form of a remote owner.
pub fn redact(value: &str) -> String {
    if value.is_empty() {
        return "[redacted]".to_string();
    }
    let digest = sha2::Sha256::digest(value.as_bytes());
    format!("[redacted:{}]", hex(&digest[..4]))
}

/// The fallback when this device's own name cannot be used.
pub const FALLBACK_DISPLAY_NAME: &str = "unknown";

/// Whether `name` may be stored and shown as a device name.
///
/// The rules are deliberately narrow, because this string is attacker-chosen
/// (it arrives in the pairing payload) and is rendered on the card a person
/// uses to decide whether to accept a pairing:
///
/// - at most [`MAX_DISPLAY_NAME_CHARS`] characters;
/// - not empty, and not only whitespace;
/// - no leading or trailing whitespace (so `"Foo "` and `"Foo"` cannot look
///   like two different devices);
/// - no control characters;
/// - no zero-width or bidirectional-format characters
///   ([`devboule_protocol::is_invisible_format`]);
/// - no line-break characters ([`devboule_protocol::is_mandatory_line_break`]),
///   which would put the card's decision text on a second line.
///
/// The fingerprint remains the check that actually matters; this only stops the
/// name from being a second, confusing channel.
pub fn validate_display_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the device name is empty".to_string());
    }
    let characters = name.chars().count();
    if characters > MAX_DISPLAY_NAME_CHARS {
        return Err(format!(
            "the device name is longer than {MAX_DISPLAY_NAME_CHARS} characters"
        ));
    }
    if name.trim() != name {
        return Err("the device name starts or ends with whitespace".to_string());
    }
    if name.trim().is_empty() {
        return Err("the device name is empty".to_string());
    }
    if let Some(category) = devboule_protocol::unsafe_character(name) {
        return Err(format!("the device name contains {category}"));
    }
    Ok(())
}

/// A name safe to publish as our own: the input when it validates, otherwise
/// [`FALLBACK_DISPLAY_NAME`].
///
/// This is not an error path. Our own name comes from the operating system, so
/// a machine with an unusable hostname must still be able to advertise itself;
/// what it must not do is send a name the other side will refuse.
pub fn display_name_or_fallback(name: &str) -> String {
    match validate_display_name(name) {
        Ok(()) => name.to_string(),
        Err(_) => FALLBACK_DISPLAY_NAME.to_string(),
    }
}
