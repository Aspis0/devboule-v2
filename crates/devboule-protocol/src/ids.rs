//! Owner identity and session identifiers.

use serde::{Deserialize, Serialize};

const SESSION_ID_MAX: usize = 64;
const OWNER_TOKEN_MAX: usize = 128;
const IDEMPOTENCY_KEY_MAX: usize = 128;

/// Who created or is driving a session.
///
/// `user` is the OS user (on Windows, the SID string). `client` distinguishes
/// two app processes of the same user. Both travel in the handshake so a
/// session id can carry owner identity without a second lookup.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct OwnerId {
    pub user: String,
    pub client: String,
}

impl OwnerId {
    pub fn new(user: impl Into<String>, client: impl Into<String>) -> Result<Self, String> {
        let user = user.into();
        let client = client.into();
        validate_owner_token(&user)?;
        validate_owner_token(&client)?;
        Ok(Self { user, client })
    }

    /// Short token embedded in a session id. Not a secret; it only names the
    /// owner so two clients of the same user do not share a bare counter.
    pub fn session_token(&self) -> String {
        let mut token = String::new();
        for (index, byte) in self.client.bytes().enumerate() {
            if index >= 16 {
                break;
            }
            if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
                token.push(byte as char);
            }
        }
        if token.is_empty() {
            "client".to_string()
        } else {
            token
        }
    }
}

/// `s.{owner}.{unique}` — owner identity is in the id, not a side table.
pub fn compose_session_id(owner: &str, unique: &str) -> Result<String, String> {
    validate_owner_token(owner)?;
    if unique.is_empty() || unique.len() > 32 || !is_id_alphabet(unique) {
        return Err("Invalid session unique component.".to_string());
    }
    let owner_token = if owner.len() > 16 {
        &owner[..16]
    } else {
        owner
    };
    let id = format!("s.{owner_token}.{unique}");
    validate_session_id(&id)?;
    Ok(id)
}

/// Validate an externally supplied session id before using it as a map key.
///
/// Accepts the M3 owner-carrying form `s.{owner}.{unique}` and the M2
/// in-process form `session-{pid}-{counter}` (and the existing test ids).
pub fn validate_session_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > SESSION_ID_MAX {
        return Err("Invalid session id.".to_string());
    }
    if is_id_alphabet(id) {
        Ok(())
    } else {
        Err("Invalid session id.".to_string())
    }
}

pub fn validate_owner_token(token: &str) -> Result<(), String> {
    if token.is_empty() || token.len() > OWNER_TOKEN_MAX {
        return Err("Invalid owner token.".to_string());
    }
    if is_id_alphabet(token) {
        Ok(())
    } else {
        Err("Invalid owner token.".to_string())
    }
}

pub fn validate_idempotency_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > IDEMPOTENCY_KEY_MAX {
        return Err("Invalid idempotency key.".to_string());
    }
    if is_id_alphabet(key) {
        Ok(())
    } else {
        Err("Invalid idempotency key.".to_string())
    }
}

fn is_id_alphabet(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_session_id_embeds_owner() {
        let id = compose_session_id("app-4242", "00000001").expect("id");
        assert!(id.starts_with("s.app-4242."));
        assert!(validate_session_id(&id).is_ok());
    }

    #[test]
    fn validate_session_id_accepts_m2_and_rejects_smuggling() {
        assert!(validate_session_id("session-123-1").is_ok());
        assert!(validate_session_id("a.b_c-2").is_ok());
        assert!(validate_session_id(&"x".repeat(64)).is_ok());
        assert!(validate_session_id("").is_err());
        assert!(validate_session_id(&"x".repeat(65)).is_err());
        assert!(validate_session_id("../other").is_err());
        assert!(validate_session_id("session id").is_err());
        assert!(validate_session_id("a:b").is_err());
    }

    #[test]
    fn owner_session_token_is_short_and_safe() {
        let owner = OwnerId::new("S-1-5-21-1-2-3-1001", "app-9999").expect("owner");
        assert_eq!(owner.session_token(), "app-9999");
    }
}
