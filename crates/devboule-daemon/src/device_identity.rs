//! This device's identity: a random `device_id`, a Noise static keypair, and
//! the display name.
//!
//! The id is what peers pin; the key is the credential bound to it, so a key
//! rotation can keep the id (`DESIGN-remote-agents.md` §8 R5). The private
//! half never touches `device.json` or the journal: it lives in the secret
//! store ([`crate::secret_store`]), and its absence is a distinct state
//! (`RemoteState::KeyMissing`) rather than a reason to mint a new key.

use std::net::IpAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use zeroize::Zeroize;

use crate::paths::RuntimePaths;
use crate::secret_store::{SecretStore, SecretStoreError};

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
                 refusing to generate a new key silently"
            ),
            Self::Envelope(message) => write!(
                formatter,
                "noise static envelope is malformed ({message}); restore the key from a backup \
                 or, if this device has no pairings worth keeping, delete device.json and the \
                 stored secret to start over"
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

/// The remote-listener state, as this daemon tracks it internally. Serialised
/// through [`RemoteState::to_wire`]: the addresses and port are part of this
/// node's reachability but do **not** belong on the wire's `remote` object
/// (brief 1b's contract puts them in `SelfInfo`).
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
    private_key: [u8; STATIC_KEY_LEN],
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

/// Read `device.json` and the private key, or create both.
///
/// A present `device.json` with an absent secret-store entry is
/// [`DeviceIdentityError::KeyMissing`]: the alternative, generating a new
/// key, would silently invalidate every peer that pinned the old one.
pub fn load_or_create(
    paths: &RuntimePaths,
    store: &dyn SecretStore,
) -> Result<DeviceIdentity, DeviceIdentityError> {
    if paths.device_file.exists() {
        return load(paths, store);
    }
    create(paths, store)
}

fn load(
    paths: &RuntimePaths,
    store: &dyn SecretStore,
) -> Result<DeviceIdentity, DeviceIdentityError> {
    let raw = std::fs::read(&paths.device_file)?;
    let file: DeviceFile = serde_json::from_slice(&raw)?;
    validate_device_file(&file)?;
    let mut stored = store
        .get(NOISE_STATIC_SECRET_NAME)?
        .ok_or(DeviceIdentityError::KeyMissing)?;
    let private_key = decode_envelope(&stored).inspect_err(|_| stored.zeroize())?;
    stored.zeroize();
    let public_key = decode_public_key(&file.public_key)?;
    Ok(DeviceIdentity {
        device_id: file.device_id,
        display_name: file.display_name,
        public_key,
        key_fingerprint: file.key_fingerprint,
        private_key,
    })
}

fn create(
    paths: &RuntimePaths,
    store: &dyn SecretStore,
) -> Result<DeviceIdentity, DeviceIdentityError> {
    let params = STATIC_KEY_PATTERN
        .parse::<snow::params::NoiseParams>()
        .map_err(|error| DeviceIdentityError::Crypto(error.to_string()))?;
    let keypair = snow::Builder::new(params)
        .generate_keypair()
        .map_err(|error| DeviceIdentityError::Crypto(error.to_string()))?;
    // `Keypair`'s two `Vec<u8>`s are heap copies of the secret and are not
    // `Zeroizing`, so both are wiped by hand on every path out of here —
    // including the error path of `to_static_key`.
    let mut private_bytes = keypair.private;
    let mut public_bytes = keypair.public;
    let keys = to_static_key(&private_bytes, "private")
        .and_then(|private| to_static_key(&public_bytes, "public").map(|public| (private, public)));
    private_bytes.zeroize();
    public_bytes.zeroize();
    let (private_key, public_key) = keys?;
    let device_id = uuid::Uuid::new_v4().to_string();
    let created_at = unix_millis();
    let display_name = hostname();
    let key_fingerprint = key_fingerprint(&public_key);
    let file = DeviceFile {
        device_id: device_id.clone(),
        created_at,
        display_name: display_name.clone(),
        public_key: base64::engine::general_purpose::STANDARD.encode(public_key),
        key_fingerprint: key_fingerprint.clone(),
    };
    // Store the private half first. A crash between these two writes leaves
    // device.json absent and the secret present, which the next start
    // overwrites; the reverse order would leave a device.json whose key
    // cannot be found (KeyMissing) on a device that never paired.
    let mut envelope = encode_envelope(&private_key);
    let stored = store.set(NOISE_STATIC_SECRET_NAME, &envelope);
    envelope.zeroize();
    stored?;
    let bytes = serde_json::to_vec_pretty(&file)?;
    crate::atomic::atomic_write(&paths.device_file, &bytes)?;
    Ok(DeviceIdentity {
        device_id,
        display_name,
        public_key,
        key_fingerprint,
        private_key,
    })
}

fn to_static_key(bytes: &[u8], which: &str) -> Result<[u8; STATIC_KEY_LEN], DeviceIdentityError> {
    bytes
        .try_into()
        .map_err(|_| DeviceIdentityError::Crypto(format!("{which} key is {} bytes", bytes.len())))
}

fn decode_public_key(encoded: &str) -> Result<[u8; STATIC_KEY_LEN], DeviceIdentityError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| DeviceIdentityError::DeviceFile(format!("publicKey: {error}")))?;
    to_static_key(&bytes, "public")
        .map_err(|_| DeviceIdentityError::DeviceFile("publicKey is not 32 bytes".to_string()))
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

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(0))
        .unwrap_or(0)
}

#[cfg(windows)]
fn hostname() -> String {
    // `COMPUTERNAME` is what the OS sets for this process; the LocalAPI is
    // never used to derive a display name, so a missing value is cosmetic.
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(not(windows))]
fn hostname() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::secret_store::InMemoryStore;

    fn tmp_paths() -> (PathBuf, RuntimePaths) {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "devboule identity {}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("runtime dir");
        (dir.clone(), RuntimePaths::from_dir(&dir))
    }

    #[test]
    fn envelope_round_trips_and_rejects_a_foreign_version() {
        let key = [7u8; STATIC_KEY_LEN];
        let envelope = encode_envelope(&key);
        assert_eq!(envelope.len(), ENVELOPE_LEN);
        assert_eq!(&envelope[..2], &ENVELOPE_VERSION);
        assert_eq!(decode_envelope(&envelope).expect("roundtrip"), key);

        let mut wrong_version = envelope;
        wrong_version[1] = 0x09;
        assert!(matches!(
            decode_envelope(&wrong_version),
            Err(DeviceIdentityError::Envelope(_))
        ));
        assert!(matches!(
            decode_envelope(&envelope[..ENVELOPE_LEN - 1]),
            Err(DeviceIdentityError::Envelope(_))
        ));
    }

    #[test]
    fn fingerprint_is_stable_and_short() {
        let key = [3u8; STATIC_KEY_LEN];
        let first = key_fingerprint(&key);
        assert_eq!(first, key_fingerprint(&key));
        assert_eq!(first.len(), 32);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let mut other = key;
        other[0] = 4;
        assert_ne!(first, key_fingerprint(&other));
    }

    #[test]
    fn create_then_load_returns_the_same_identity() {
        let (dir, paths) = tmp_paths();
        let store = InMemoryStore::default();
        let created = load_or_create(&paths, &store).expect("create");
        assert!(!created.device_id.is_empty());
        assert!(uuid::Uuid::parse_str(&created.device_id).is_ok());
        assert_eq!(
            created.key_fingerprint,
            key_fingerprint(&created.public_key)
        );

        let loaded = load_or_create(&paths, &store).expect("load");
        assert_eq!(loaded.device_id, created.device_id);
        assert_eq!(loaded.public_key, created.public_key);
        assert_eq!(loaded.key_fingerprint, created.key_fingerprint);

        // `createdAt` is provenance carried by the file; it is read back by the
        // validator and must survive the round trip.
        let raw = std::fs::read(&paths.device_file).expect("device.json");
        let parsed: DeviceFile = serde_json::from_slice(&raw).expect("json");
        assert!(parsed.created_at > 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn device_json_schema_is_the_documented_one() {
        let (dir, paths) = tmp_paths();
        let store = InMemoryStore::default();
        let identity = load_or_create(&paths, &store).expect("create");
        let raw = std::fs::read_to_string(&paths.device_file).expect("device.json");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("json");

        for key in [
            "deviceId",
            "createdAt",
            "displayName",
            "publicKey",
            "keyFingerprint",
        ] {
            assert!(value.get(key).is_some(), "device.json is missing {key}");
        }
        assert_eq!(
            value["deviceId"].as_str(),
            Some(identity.device_id.as_str())
        );
        assert!(value["createdAt"].as_u64().is_some());
        assert!(value["displayName"].as_str().is_some());
        let public_key = value["publicKey"].as_str().expect("publicKey");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(public_key)
            .expect("base64");
        assert_eq!(decoded.len(), STATIC_KEY_LEN);
        assert_eq!(
            value["keyFingerprint"].as_str(),
            Some(identity.key_fingerprint.as_str())
        );
        // The private half is never in the file.
        assert!(!raw.contains("private"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_present_device_file_without_a_stored_key_is_key_missing() {
        let (dir, paths) = tmp_paths();
        let store = InMemoryStore::default();
        let identity = load_or_create(&paths, &store).expect("create");
        // A different, empty store stands in for "the credential is gone":
        // simulating that by deleting is not available (the store has no
        // delete), and an empty store is the stronger case anyway — the key is
        // absent everywhere, and `device.json` is still on disk.
        let empty = InMemoryStore::default();
        match load_or_create(&paths, &empty) {
            Err(DeviceIdentityError::KeyMissing) => {}
            Err(other) => panic!("expected KeyMissing, got {other:?}"),
            Ok(_) => panic!("a present device.json with no stored key must not load"),
        }
        // And the missing key is not silently replaced.
        assert!(load_or_create(&paths, &empty).is_err());
        assert_eq!(identity.device_id, read_id(&paths));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn read_id(paths: &RuntimePaths) -> String {
        let raw = std::fs::read_to_string(&paths.device_file).expect("device.json");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("json");
        value["deviceId"].as_str().expect("deviceId").to_string()
    }

    #[test]
    fn redact_does_not_reproduce_the_value() {
        let secret = "nxd5gUfvzj11CNTRL";
        let redacted = redact(secret);
        assert!(!redacted.contains(secret));
        assert!(redacted.starts_with("[redacted:"));
        assert_eq!(redacted, redact(secret));
    }

    #[test]
    fn remote_state_projects_onto_the_wire_shape() {
        let enabled = RemoteState::Enabled {
            addresses: vec!["100.64.0.1".parse().expect("ip")],
            port: 47831,
        };
        assert_eq!(
            serde_json::to_value(enabled.to_wire()).expect("json")["state"],
            "enabled"
        );
        // Addresses and port describe this node's reachability; they belong
        // to `SelfInfo`, not to the wire `remote` object.
        assert_eq!(enabled.addresses(), vec!["100.64.0.1".to_string()]);
        assert_eq!(enabled.port(), Some(47831));

        assert_eq!(
            serde_json::to_value(RemoteState::Disabled("no tailscale".into()).to_wire())
                .expect("json")["state"],
            "disabled"
        );
        assert_eq!(
            serde_json::to_value(RemoteState::KeyMissing.to_wire()).expect("json")["state"],
            "key_missing"
        );
        assert!(RemoteState::KeyMissing.addresses().is_empty());
        assert_eq!(RemoteState::KeyMissing.port(), None);
    }

    #[test]
    fn a_malformed_envelope_is_not_reported_as_key_missing() {
        // F4: a truncated or version-shifted secret must fail loudly with the
        // store kind and the remedy in the message. Mapping it to
        // `KeyMissing` would make the daemon mint a new identity and orphan
        // every pairing this device has.
        for bytes in [vec![], vec![0u8; 33], vec![0x00, 0x09], vec![0xff; 34]] {
            let error = decode_envelope(&bytes).expect_err("malformed envelope");
            assert!(matches!(error, DeviceIdentityError::Envelope(_)));
            let rendered = error.to_string();
            assert!(
                rendered.contains("device.json") && rendered.contains("secret"),
                "the message must name both halves of the state: {rendered}"
            );
        }

        let (dir, paths) = tmp_paths();
        let store = InMemoryStore::default();
        load_or_create(&paths, &store).expect("create");
        store
            .set(NOISE_STATIC_SECRET_NAME, &[0u8; 33])
            .expect("truncate the stored secret");
        match load_or_create(&paths, &store) {
            Err(DeviceIdentityError::Envelope(message)) => {
                assert!(message.contains("expected 34 bytes, got 33"), "{message}");
            }
            Err(other) => panic!("a malformed secret must not be KeyMissing: {other}"),
            Ok(_) => panic!("a malformed secret must not mint a new identity"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_uuid_device_id_is_refused() {
        let file = DeviceFile {
            device_id: "not-a-uuid".to_string(),
            created_at: 1,
            display_name: "host".to_string(),
            public_key: base64::engine::general_purpose::STANDARD.encode([0u8; 32]),
            key_fingerprint: key_fingerprint(&[0u8; 32]),
        };
        assert!(matches!(
            validate_device_file(&file),
            Err(DeviceIdentityError::DeviceFile(_))
        ));
    }

    #[test]
    fn a_zero_created_at_is_refused_as_not_written_by_this_daemon() {
        let file = DeviceFile {
            device_id: uuid::Uuid::new_v4().to_string(),
            created_at: 0,
            display_name: "host".to_string(),
            public_key: base64::engine::general_purpose::STANDARD.encode([0u8; 32]),
            key_fingerprint: key_fingerprint(&[0u8; 32]),
        };
        assert!(matches!(
            validate_device_file(&file),
            Err(DeviceIdentityError::DeviceFile(_))
        ));
    }
}
