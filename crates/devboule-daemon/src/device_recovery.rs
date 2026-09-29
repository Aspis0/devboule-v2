//! Rebuild the device identity file when it is lost or unreadable.
//!
//! One responsibility: bring back a working `device.json` from the private
//! key that survives in the secret store. Peers pin the public key, not the
//! device id, so a rebuilt file (fresh id, same key) keeps every pairing
//! authenticating. The stored key is read, never replaced and never rewritten.

use std::path::{Path, PathBuf};

use base64::Engine;
use zeroize::Zeroize;

use crate::device_identity::{
    created_at_stamp, decode_envelope, display_name_or_fallback, hostname, key_fingerprint,
    stored_key, unix_millis, DeviceFile, DeviceIdentity, DeviceIdentityError, STATIC_KEY_LEN,
};
use crate::paths::RuntimePaths;
use crate::secret_store::SecretStore;

/// Write a fresh `device.json` for the private key already in the store.
///
/// The read goes through the same combiner as every other store read, so a
/// store failure here names the file state too instead of surfacing a bare
/// credential-store sentence. A malformed stored secret fails loudly instead
/// of minting: without the key there is nothing to rebuild from. A failed
/// file write changes nothing but the absence it found, so the next start
/// retries the same rebuild.
pub(crate) fn rebuild_identity(
    paths: &RuntimePaths,
    store: &dyn SecretStore,
    preferred_display_name: Option<&str>,
    file_state: &str,
) -> Result<DeviceIdentity, DeviceIdentityError> {
    let mut stored = stored_key(store, file_state)?.ok_or(DeviceIdentityError::KeyMissing)?;
    let private_key = decode_envelope(&stored).inspect_err(|_| stored.zeroize())?;
    stored.zeroize();
    let public_key = derive_public_key(&private_key);
    let device_id = uuid::Uuid::new_v4().to_string();
    let display_name = preferred_display_name
        .map(display_name_or_fallback)
        .unwrap_or_else(|| display_name_or_fallback(&hostname()));
    let key_fingerprint = key_fingerprint(&public_key);
    let file = DeviceFile {
        device_id: device_id.clone(),
        created_at: created_at_stamp(),
        display_name: display_name.clone(),
        public_key: base64::engine::general_purpose::STANDARD.encode(public_key),
        key_fingerprint: key_fingerprint.clone(),
    };
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

/// The X25519 public half of a stored private key: the same scalar
/// multiplication the Noise handshake performs, so a rebuilt file
/// authenticates as the key peers pinned.
pub(crate) fn derive_public_key(private_key: &[u8; STATIC_KEY_LEN]) -> [u8; STATIC_KEY_LEN] {
    curve25519_dalek::MontgomeryPoint::mul_base_clamped(*private_key).to_bytes()
}

/// Rename an unreadable `device.json` aside before it is rebuilt over.
///
/// Best effort by construction: the rebuild overwrites the original either
/// way, so this returns the sidecar path whether the salvage succeeded or
/// not, and the heal never waits on it.
pub(crate) fn move_aside(paths: &RuntimePaths) -> PathBuf {
    let target = paths
        .dir
        .join(format!("device.json.unreadable-{}", unix_millis()));
    salvage(&paths.device_file, &target);
    target
}

/// Best-effort salvage of unreadable bytes: rename, else copy, else note on
/// stderr. Infallible — the rebuild overwrites the original either way, so a
/// failed salvage costs a copy, never the identity. (The daemon log sink
/// owns these notes once U4 lands; until then they ride stderr like the other
/// load-time lines.)
fn salvage(source: &Path, target: &Path) {
    let rename_error = match std::fs::rename(source, target) {
        Ok(()) => return,
        Err(error) => error,
    };
    let copy_error = match std::fs::read(source).and_then(|bytes| std::fs::write(target, &bytes)) {
        Ok(()) => return,
        Err(error) => error,
    };
    eprintln!(
        "device identity: could not salvage {} to {} (rename: {rename_error}, copy: \
         {copy_error}); rebuilding without a salvage copy",
        source.display(),
        target.display()
    );
}

/// A display name salvaged from a parsed-but-wrong document.
///
/// Best-effort on purpose: a document can hold a whole `displayName` string
/// where the surrounding schema is wrong. Anything less than a string is
/// nothing — the caller falls back to the hostname.
pub(crate) fn lenient_display_name(value: &serde_json::Value) -> Option<String> {
    value.get("displayName")?.as_str().map(str::to_string)
}

#[cfg(test)]
#[path = "device_recovery_tests.rs"]
mod tests;
