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
    ///
    /// A remote owner (`peer_<device_id>`, `devboule-daemon/src/peer_policy.rs`)
    /// gets a `p` prefix so a session id of remote origin is recognisable and
    /// cannot collide with `process-<pid>` or `app-<pid>`. The token is
    /// cosmetic: authority is [`OwnerId::user`].
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
            token = "client".to_string();
        }
        if self.user.starts_with("peer_") {
            format!("p{token}")
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

    #[test]
    fn a_remote_owner_gets_a_p_prefixed_token_that_cannot_collide() {
        // Design §8b A2. The prefix is cosmetic; the point of the test is that
        // the two namespaces (`process-*`/`app-*` and `peer_*`) cannot produce
        // the same token, and that the result is a valid session id.
        let remote = OwnerId::new("peer_dev-1", "client").expect("remote owner");
        let daemon = OwnerId::new("peer_dev-1", "daemon").expect("remote owner");
        assert_eq!(remote.session_token(), "pclient");
        assert_eq!(daemon.session_token(), "pdaemon");

        // No local client label can produce the same token. Note that the
        // `p` prefix is *not* a namespace guarantee on its own: the local
        // label `process-1` legitimately yields the token `process-1`, which
        // also starts with `p`. What rules out collision is that a remote
        // token is `p` + a role name, and no local token is.
        let remote_tokens = [remote.session_token(), daemon.session_token()];
        assert_eq!(
            remote_tokens,
            ["pclient".to_string(), "pdaemon".to_string()]
        );
        for local in ["process-1", "app-1", "client"] {
            let owner = OwnerId::new("S-1-5-21-1", local).expect("local owner");
            assert!(
                !remote_tokens.contains(&owner.session_token()),
                "local token {} collides with a remote token",
                owner.session_token()
            );
            assert_eq!(owner.session_token(), local);
        }

        let id = compose_session_id(&remote.session_token(), "00000001").expect("compose");
        assert!(validate_session_id(&id).is_ok(), "{id}");
        assert!(id.starts_with("s.pclient."), "{id}");
    }
}
