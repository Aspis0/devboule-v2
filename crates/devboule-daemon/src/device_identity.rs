//! This device's identity: a random `device_id`, a Noise static keypair, and
//! the display name.
//!
//! The id is what peers pin; the key is the credential bound to it, so a key
//! rotation can keep the id (`DESIGN-remote-agents.md` §8 R5). The private
//! half never touches `device.json` or the journal: it lives in the secret
//! store ([`crate::secret_store`]), and its absence is a distinct state
//! (`RemoteState::KeyMissing`) rather than a reason to mint a new key.

use std::net::IpAddr;

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use zeroize::Zeroize;

use crate::secret_store::SecretStoreError;

#[path = "device_identity_policy.rs"]
mod device_identity_policy;
#[path = "device_identity_store.rs"]
mod device_identity_store;

pub use device_identity_policy::{display_name_or_fallback, redact, validate_display_name};
pub use device_identity_store::load_or_create;
pub(crate) use device_identity_store::{created_at_stamp, hostname, stored_key, unix_millis};

/// Secret-store entry name. The keyring maps this to
/// `noise-static-<runtime dir hash>`; the file store to
/// `<runtime dir>/secrets/noise-static.bin`.
pub const NOISE_STATIC_SECRET_NAME: &str = "noise-static";

/// `0x00 0x01` followed by the 32-byte X25519 private key.
pub const ENVELOPE_VERSION: [u8; 2] = [0x00, 0x01];
pub const ENVELOPE_LEN: usize = 34;
pub const STATIC_KEY_LEN: usize = 32;

/// The Noise pattern the static keypair is generated for. Key generation does
/// not depend on the pattern, but the params are the parsed name this crate
/// uses everywhere else, so a typo fails here once instead of at every
/// handshake.
const STATIC_KEY_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

/// Longest display name this daemon accepts from a peer or publishes itself.
///
/// A hostname is at most 63 characters by DNS rules, so 64 never truncates a
/// legitimate name; it is short enough that the confirm card cannot be filled
/// with text and short enough that a stored row stays small.
pub const MAX_DISPLAY_NAME_CHARS: usize = 64;

#[derive(Debug)]
pub enum DeviceIdentityError {
    Io(String),
    Json(String),
    Secret(SecretStoreError),
    KeyMissing,
    /// The stored bytes are not a valid envelope: wrong length or an unknown
    /// version. Distinct from `KeyMissing` so a truncated file fails loudly
    /// instead of being read as "no key yet", which would mint a new identity
    /// and silently orphan every pairing this device has.
    Envelope(String),
    Crypto(String),
    DeviceFile(String),
}

impl std::fmt::Display for DeviceIdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) => write!(formatter, "device identity io: {message}"),
            Self::Json(message) => write!(formatter, "device identity json: {message}"),
            Self::Secret(error) => write!(formatter, "device identity secret store: {error}"),
            Self::KeyMissing => write!(
                formatter,
                "device.json exists but the Noise static key is not in the secret store; \
                 refusing to generate a new key silently, which would orphan every pairing. \
                 Restore the key from a backup, or delete device.json and restart to start \
                 over — the pairings will have to be made again"
            ),
            Self::Envelope(message) => write!(
                formatter,
                "the stored secret for device.json is malformed ({message}); restore the key \
                 from a backup — without it this device cannot authenticate to its pairings"
            ),
            Self::Crypto(message) => write!(formatter, "noise static key: {message}"),
            Self::DeviceFile(message) => write!(formatter, "device.json: {message}"),
        }
    }
}

impl std::error::Error for DeviceIdentityError {}

impl From<std::io::Error> for DeviceIdentityError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<serde_json::Error> for DeviceIdentityError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error.to_string())
    }
}

impl From<SecretStoreError> for DeviceIdentityError {
    fn from(error: SecretStoreError) -> Self {
        Self::Secret(error)
    }
}

/// What `device.json` holds. `public_key` is base64 (the 32 raw bytes do not
/// survive a round trip through JSON text as text); `key_fingerprint` is the
/// short human-comparable label.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceFile {
    pub device_id: String,
    pub created_at: u64,
    pub display_name: String,
    pub public_key: String,
    pub key_fingerprint: String,
}

fn validate_device_file(file: &DeviceFile) -> Result<(), DeviceIdentityError> {
    if file.device_id.is_empty() {
        return Err(DeviceIdentityError::DeviceFile(
            "deviceId is empty".to_string(),
        ));
    }
    if uuid::Uuid::parse_str(&file.device_id).is_err() {
        return Err(DeviceIdentityError::DeviceFile(
            "deviceId is not a UUID".to_string(),
        ));
    }
    // `createdAt` is provenance, not a control, but a zero value means the file
    // was written by something that is not this daemon. Reading it here is what
    // keeps the field load-bearing instead of decoration.
    if file.created_at == 0 {
        return Err(DeviceIdentityError::DeviceFile(
            "createdAt is zero, which this daemon never writes".to_string(),
        ));
    }
    Ok(())
}

/// The remote-listener state, as this daemon tracks it internally. Serialised
/// through [`RemoteState::to_wire`]: the addresses and port are part of this
/// node's reachability but do **not** belong on the wire's `remote` object
/// (`SelfInfo` carries them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteState {
    Disabled(String),
    KeyMissing,
    Enabled { addresses: Vec<IpAddr>, port: u16 },
}

impl RemoteState {
    /// The wire shape: `{ state, reason }` and nothing else.
    pub fn to_wire(&self) -> devboule_protocol::RemoteState {
        match self {
            Self::Disabled(reason) => devboule_protocol::RemoteState::disabled(reason.clone()),
            Self::KeyMissing => devboule_protocol::RemoteState::key_missing(),
            Self::Enabled { .. } => devboule_protocol::RemoteState::enabled(),
        }
    }

    pub fn addresses(&self) -> Vec<String> {
        match self {
            Self::Enabled { addresses, .. } => addresses
                .iter()
                .map(|address| address.to_string())
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn port(&self) -> Option<u16> {
        match self {
            Self::Enabled { port, .. } => Some(*port),
            _ => None,
        }
    }
}

/// A device identity loaded from disk plus the private key loaded from the
/// secret store. The private key is zeroized on drop.
pub struct DeviceIdentity {
    pub device_id: String,
    pub display_name: String,
    pub public_key: [u8; STATIC_KEY_LEN],
    pub key_fingerprint: String,
    // Crate-visible so the recovery module can build the identity its rebuild
    // returns; read nowhere else — the key is borrowed through `private_key()`.
    pub(crate) private_key: [u8; STATIC_KEY_LEN],
}

impl Drop for DeviceIdentity {
    fn drop(&mut self) {
        self.private_key.zeroize();
    }
}

impl DeviceIdentity {
    /// The static private key, for the Noise builder. Borrowed, never cloned
    /// into a long-lived value.
    pub fn private_key(&self) -> &[u8; STATIC_KEY_LEN] {
        &self.private_key
    }

    pub fn public_key_b64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.public_key)
    }
}

/// `0x00 0x01` + the 32-byte key. The version prefix is what lets a future
/// envelope change be detected instead of silently misread as a key.
pub fn encode_envelope(private_key: &[u8; STATIC_KEY_LEN]) -> [u8; ENVELOPE_LEN] {
    let mut envelope = [0u8; ENVELOPE_LEN];
    envelope[..2].copy_from_slice(&ENVELOPE_VERSION);
    envelope[2..].copy_from_slice(private_key);
    envelope
}

pub fn decode_envelope(bytes: &[u8]) -> Result<[u8; STATIC_KEY_LEN], DeviceIdentityError> {
    if bytes.len() != ENVELOPE_LEN {
        return Err(DeviceIdentityError::Envelope(format!(
            "expected {ENVELOPE_LEN} bytes, got {}",
            bytes.len()
        )));
    }
    if bytes[..2] != ENVELOPE_VERSION {
        return Err(DeviceIdentityError::Envelope(format!(
            "unsupported envelope version {:02x}{:02x}",
            bytes[0], bytes[1]
        )));
    }
    let mut key = [0u8; STATIC_KEY_LEN];
    key.copy_from_slice(&bytes[2..]);
    Ok(key)
}

/// Hex of the first 16 bytes of SHA-256 of the public key. Short enough to
/// read to another person, long enough that a collision is not a practical
/// concern.
pub fn key_fingerprint(public_key: &[u8]) -> String {
    let digest = sha2::Sha256::digest(public_key);
    hex(&digest[..16])
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
#[path = "device_identity_tests.rs"]
mod tests;
