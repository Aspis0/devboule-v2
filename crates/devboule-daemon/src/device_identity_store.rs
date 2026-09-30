//! Identity persistence: load `device.json` and the stored private key,
//! create both on first run, and heal a missing or damaged file — the
//! rebuild and move-aside themselves live in `crate::device_recovery`.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use zeroize::Zeroize;

use super::device_identity_policy::display_name_or_fallback;
use super::{
    decode_envelope, encode_envelope, key_fingerprint, validate_device_file, DeviceFile,
    DeviceIdentity, DeviceIdentityError, NOISE_STATIC_SECRET_NAME, STATIC_KEY_LEN,
    STATIC_KEY_PATTERN,
};
use crate::paths::RuntimePaths;
use crate::secret_store::SecretStore;

/// Read `device.json` and the private key, or create both.
///
/// One rule: a stored key is never replaced. A missing file beside a stored
/// key is rebuilt from it (fresh id, same key — peers pin the public key, so
/// the pairings keep authenticating); a new keypair is minted only when the
/// store holds no key. A blank or unparseable file beside a stored key is
/// moved aside first and then rebuilt the same way, so nothing restorable is
/// lost. Without a stored key there is nothing to rebuild from: blank refuses
/// with the file delete as its recovery, and an unparseable file stays a
/// parse error.
///
/// A leading UTF-8 BOM on a real document is stripped before parsing.
pub fn load_or_create(
    paths: &RuntimePaths,
    store: &dyn SecretStore,
) -> Result<DeviceIdentity, DeviceIdentityError> {
    if paths.device_file.exists() {
        return load(paths, store);
    }
    match stored_key(store, "the identity file is missing")? {
        Some(_) => crate::device_recovery::rebuild_identity(
            paths,
            store,
            None,
            "the identity file is missing",
        ),
        None => create(paths, store),
    }
}

/// device.json files this daemon writes are a few hundred bytes; anything
/// past the cap is damage, and bounding the read bounds every parse below.
const MAX_DEVICE_FILE_BYTES: u64 = 64 * 1024;

/// The stored private-key envelope, if any. A store failure names both facts —
/// what the file turned out to be and why the key could not be checked — so a
/// keyring outage never hides behind, or hides, the file diagnosis.
pub(crate) fn stored_key(
    store: &dyn SecretStore,
    file_state: &str,
) -> Result<Option<Vec<u8>>, DeviceIdentityError> {
    store.get(NOISE_STATIC_SECRET_NAME).map_err(|error| {
        DeviceIdentityError::DeviceFile(format!(
            "{file_state}, and the stored key could not be read ({error})"
        ))
    })
}

fn load(
    paths: &RuntimePaths,
    store: &dyn SecretStore,
) -> Result<DeviceIdentity, DeviceIdentityError> {
    // Bounded like every other config read, through the same helper: the
    // tri-state tells a first run from damage, and oversize or UTF-16 refuse
    // here with their own prescription instead of reaching any branch below.
    let bytes =
        match crate::config_read::read_config_file(&paths.device_file, MAX_DEVICE_FILE_BYTES)? {
            crate::config_read::ConfigFile::Absent => {
                return Err(DeviceIdentityError::Io(
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "device.json vanished during startup; restart to retry",
                    )
                    .to_string(),
                ));
            }
            crate::config_read::ConfigFile::Blank => {
                // The recovery names only what exists: with no stored key there is
                // nothing to rebuild from — deleting the file and restarting mints a
                // first-run identity.
                return match stored_key(store, "the identity file holds no readable bytes")? {
                    None => Err(DeviceIdentityError::DeviceFile(
                        "the identity file exists but holds no identity; delete the file and \
                         restart to create a fresh identity"
                            .to_string(),
                    )),
                    Some(_) => {
                        crate::device_recovery::move_aside(paths);
                        crate::device_recovery::rebuild_identity(
                            paths,
                            store,
                            None,
                            "the identity file holds no readable bytes",
                        )
                    }
                };
            }
            crate::config_read::ConfigFile::Present(bytes) => bytes,
        };
    // One parse from bytes; the Value serves both the schema read and the
    // salvaged name, so a damaged file is never deserialized twice.
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => {
            return heal_unparseable(paths, store, None, error);
        }
    };
    let name = crate::device_recovery::lenient_display_name(&value);
    let file: DeviceFile = match serde_json::from_value(value) {
        Ok(file) => file,
        Err(error) => {
            return heal_unparseable(paths, store, name, error);
        }
    };
    validate_device_file(&file).map_err(unusable_file)?;
    let public_key = decode_public_key(&file.public_key).map_err(unusable_file)?;
    let mut stored = stored_key(store, "the identity file holds an identity")?
        .ok_or(DeviceIdentityError::KeyMissing)?;
    let private_key = decode_envelope(&stored).inspect_err(|_| stored.zeroize())?;
    stored.zeroize();
    Ok(DeviceIdentity {
        device_id: file.device_id,
        // Sanitised on the way in as well: a hand-edited `device.json` is one of
        // the ways a bad name could otherwise reach `SelfInfo` and every future
        // pairing payload.
        display_name: display_name_or_fallback(&file.display_name),
        public_key,
        key_fingerprint: file.key_fingerprint,
        private_key,
    })
}

/// The unreadable-file branch, shared by garbage bytes and wrong-shape
/// documents: heal around a stored key (salvaging the name when there is
/// one); without a key the file is all there is, so refuse with what to do.
fn heal_unparseable(
    paths: &RuntimePaths,
    store: &dyn SecretStore,
    name: Option<String>,
    error: serde_json::Error,
) -> Result<DeviceIdentity, DeviceIdentityError> {
    match stored_key(store, "the identity file cannot be parsed as an identity")? {
        None => Err(DeviceIdentityError::Json(format!(
            "{error}; restore it from a backup, or delete the file and restart to create a \
             fresh identity"
        ))),
        Some(_) => {
            crate::device_recovery::move_aside(paths);
            crate::device_recovery::rebuild_identity(
                paths,
                store,
                name.as_deref(),
                "the identity file cannot be parsed as an identity",
            )
        }
    }
}

/// A readable-but-wrong file refuses with its evidence intact; deleting it
/// heals either way (an absent file rebuilds from the key, or mints without
/// one), so the refusal names that recovery.
fn unusable_file(error: DeviceIdentityError) -> DeviceIdentityError {
    match error {
        DeviceIdentityError::DeviceFile(message) => DeviceIdentityError::DeviceFile(format!(
            "{message}; delete the file and restart to create a working identity"
        )),
        other => other,
    }
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
    let created_at = created_at_stamp();
    // Sanitised, not rejected: an unusable hostname must not stop the daemon
    // from having an identity, and it must not put a name on the wire that the far
    // side will refuse.
    let display_name = display_name_or_fallback(&hostname());
    let key_fingerprint = key_fingerprint(&public_key);
    let file = DeviceFile {
        device_id: device_id.clone(),
        created_at,
        display_name: display_name.clone(),
        public_key: base64::engine::general_purpose::STANDARD.encode(public_key),
        key_fingerprint: key_fingerprint.clone(),
    };
    // Store the private half first. A crash between these two writes leaves
    // device.json absent and the secret present, which the next start rebuilds
    // from the stored key; the reverse order would leave a device.json whose
    // key cannot be found (KeyMissing) on a device that never paired,
    // recoverable only by deleting the file.
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

pub(crate) fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(0))
        .unwrap_or(0)
}

/// The `created_at` this daemon writes.
///
/// Never zero, even on a clock that is set before 1970: `validate_device_file`
/// rejects `createdAt == 0` as "not written by this daemon", so writing a zero
/// would produce a `device.json` that this same loader refuses on every start —
/// a machine with a badly skewed clock would be unable to use its own identity
/// until the clock was fixed. One millisecond past the epoch is a value
/// the validator accepts and no real daemon start can produce by accident.
pub(crate) fn created_at_stamp() -> u64 {
    unix_millis().max(1)
}

#[cfg(windows)]
pub(crate) fn hostname() -> String {
    // `COMPUTERNAME` is what the OS sets for this process.
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// The machine's name on platforms that are not Windows yet.
///
/// `HOSTNAME` is a shell variable: a GUI-launched process normally does not have
/// it, which is why reading the file the OS keeps matters. That string is not
/// cosmetic — it is the half of the pairing confirmation card that is not the
/// fingerprint — so the file the OS keeps is read as a fallback. The daemon
/// does not run on these platforms yet, and no new dependency is added for it.
#[cfg(not(windows))]
pub(crate) fn hostname() -> String {
    for path in ["/etc/hostname", "/proc/sys/kernel/hostname"] {
        if let Ok(name) = std::fs::read_to_string(path) {
            let name = name.trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string())
}
