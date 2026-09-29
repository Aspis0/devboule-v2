//! Tests for the identity recovery: rebuilds around a stored key, the
//! move-aside of unreadable files, and the failures that must stay loud.

use std::path::PathBuf;

use super::*;
use crate::device_identity::{
    decode_envelope, display_name_or_fallback, hostname, load_or_create, validate_display_name,
    DeviceIdentityError, NOISE_STATIC_SECRET_NAME,
};
use crate::secret_store::{InMemoryStore, SecretStore, SecretStoreError};

fn tmp_paths() -> (PathBuf, crate::paths::RuntimePaths) {
    let dir = crate::test_dirs::test_temp_dir("devboule recovery");
    (dir.clone(), crate::paths::RuntimePaths::from_dir(&dir))
}

fn live_key(store: &InMemoryStore) -> Vec<u8> {
    store
        .get(NOISE_STATIC_SECRET_NAME)
        .expect("read")
        .expect("the key is live")
}

fn sidecars(dir: &std::path::Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .expect("list")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("device.json.unreadable-"))
        })
        .collect()
}

#[test]
fn a_missing_device_file_with_a_stored_key_rebuilds_around_it() {
    let (dir, paths) = tmp_paths();
    let store = InMemoryStore::default();
    let created = load_or_create(&paths, &store).expect("create");
    let key_before = live_key(&store);
    std::fs::remove_file(&paths.device_file).expect("remove");

    // One rule: a stored key is never replaced. The file is rebuilt from it —
    // fresh id, same key — whatever trusted it before keeps authenticating.
    let rebuilt = load_or_create(&paths, &store).expect("rebuild heals");
    assert_eq!(
        live_key(&store),
        key_before,
        "the stored key is never replaced"
    );
    assert_eq!(
        rebuilt.public_key, created.public_key,
        "the trusted key survives the lost file"
    );
    assert_ne!(
        rebuilt.device_id, created.device_id,
        "the unrecoverable id is fresh"
    );
    assert_eq!(rebuilt.key_fingerprint, created.key_fingerprint);
    let again = load_or_create(&paths, &store).expect("stable");
    assert_eq!(again.device_id, rebuilt.device_id);
    assert_eq!(again.public_key, created.public_key);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_wiped_runtime_dir_keeps_its_key() {
    // The wipe: the runtime directory's contents are gone while the keyring
    // entry survives. A key this directory never saw a file for must still
    // survive — it is byte-identical to a genuine first run with a leftover
    // entry, and the loud default is the rebuild, not the mint.
    let (dir_a, paths_a) = tmp_paths();
    let store_a = InMemoryStore::default();
    let created = load_or_create(&paths_a, &store_a).expect("create");
    let key_bytes = live_key(&store_a);

    let dir_b = crate::test_dirs::test_temp_dir("devboule recovery wipe");
    let paths_b = crate::paths::RuntimePaths::from_dir(&dir_b);
    let store_b = InMemoryStore::default();
    store_b
        .set(NOISE_STATIC_SECRET_NAME, &key_bytes)
        .expect("transplant");
    assert!(
        !paths_b.device_file.exists(),
        "this directory never held a file"
    );

    let rebuilt = load_or_create(&paths_b, &store_b).expect("rebuild heals");
    assert_eq!(live_key(&store_b), key_bytes, "the key is never replaced");
    assert_eq!(rebuilt.public_key, created.public_key);
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

#[test]
fn a_create_write_failure_heals_on_the_next_start() {
    // `create` stores the secret before writing the file; a failed file write
    // leaves exactly "file absent, key present". Deleting the file after a
    // good create reproduces that window byte for byte.
    let (dir, paths) = tmp_paths();
    let store = InMemoryStore::default();
    let created = load_or_create(&paths, &store).expect("create");
    let key_before = live_key(&store);
    std::fs::remove_file(&paths.device_file).expect("the write failed");

    let healed =
        load_or_create(&paths, &store).expect("the next start ends with a working identity");
    assert_eq!(live_key(&store), key_before, "the key survives the crash");
    assert_eq!(healed.public_key, created.public_key);
    let again = load_or_create(&paths, &store).expect("stable");
    assert_eq!(again.device_id, healed.device_id);
    assert_eq!(again.public_key, healed.public_key);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_blank_device_file_with_a_stored_key_moves_aside_and_rebuilds() {
    for (tag, bytes) in [
        ("empty", b"".as_slice()),
        ("whitespace", b"  \n ".as_slice()),
        ("bom-only", b"\xef\xbb\xbf".as_slice()),
    ] {
        let dir = crate::test_dirs::test_temp_dir(&format!("devboule-recovery-{tag}"));
        let paths = crate::paths::RuntimePaths::from_dir(&dir);
        let store = InMemoryStore::default();
        let created = load_or_create(&paths, &store).expect("create");
        let key_before = live_key(&store);
        std::fs::write(&paths.device_file, bytes).expect("truncate");

        let rebuilt = load_or_create(&paths, &store).expect("{tag}: blank heals");
        assert_eq!(live_key(&store), key_before, "{tag}: the key is kept");
        assert_eq!(rebuilt.public_key, created.public_key, "{tag}: same key");
        let found = sidecars(&dir);
        assert_eq!(found.len(), 1, "{tag}: one sidecar");
        assert_eq!(
            std::fs::read(&found[0]).expect("bytes"),
            bytes,
            "{tag}: the bad bytes are kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn an_unparseable_device_file_with_a_stored_key_moves_aside_and_rebuilds() {
    let (dir, paths) = tmp_paths();
    let store = InMemoryStore::default();
    let created = load_or_create(&paths, &store).expect("create");
    let key_before = live_key(&store);
    // Valid JSON with a whole display name in it, but not an identity: the
    // name must survive while the damage is moved aside.
    let damaged = br#"{"displayName": "Custom Name", "createdAt": "yesterday"}"#;
    std::fs::write(&paths.device_file, damaged).expect("damage");

    let rebuilt = load_or_create(&paths, &store).expect("heals");
    assert_eq!(live_key(&store), key_before, "the key is kept");
    assert_eq!(rebuilt.public_key, created.public_key, "same key");
    assert_eq!(
        rebuilt.display_name, "Custom Name",
        "a salvaged name survives the rebuild"
    );
    let found = sidecars(&dir);
    assert_eq!(found.len(), 1, "one sidecar");
    assert_eq!(
        std::fs::read(&found[0]).expect("bytes"),
        damaged,
        "the bad bytes are kept"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_garbage_device_file_with_a_stored_key_heals_without_a_name() {
    // Garbage bytes parse as nothing, so there is no name to salvage — but
    // the heal does not need one.
    let (dir, paths) = tmp_paths();
    let store = InMemoryStore::default();
    let created = load_or_create(&paths, &store).expect("create");
    let damaged = b"{ this is not json";
    std::fs::write(&paths.device_file, damaged).expect("damage");

    let rebuilt = load_or_create(&paths, &store).expect("heals");
    assert_eq!(rebuilt.public_key, created.public_key, "same key");
    assert_eq!(
        rebuilt.display_name,
        display_name_or_fallback(&hostname()),
        "the hostname fallback, exactly"
    );
    let found = sidecars(&dir);
    assert_eq!(found.len(), 1, "one sidecar");
    assert_eq!(
        std::fs::read(&found[0]).expect("bytes"),
        damaged,
        "the bad bytes are kept"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rebuild_without_a_salvageable_name_takes_the_hostname() {
    // A name that is not a string is nothing: the rebuild falls back to the
    // hostname rather than inventing one.
    assert_eq!(
        lenient_display_name(&serde_json::json!({"displayName": 42})),
        None
    );
    assert_eq!(lenient_display_name(&serde_json::json!([1, 2])), None);
    assert_eq!(
        lenient_display_name(
            &serde_json::json!({"displayName": "Custom Name", "createdAt": "yesterday"})
        ),
        Some("Custom Name".to_string())
    );

    let (dir, paths) = tmp_paths();
    let store = InMemoryStore::default();
    load_or_create(&paths, &store).expect("create");
    std::fs::write(&paths.device_file, br#"{"displayName": 42}"#).expect("damage");
    let rebuilt = load_or_create(&paths, &store).expect("heals");
    validate_display_name(&rebuilt.display_name).expect("a usable name");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_oversize_device_file_refuses_even_with_a_stored_key() {
    // Past the cap the file is damage no branch heals: loud, with the cap in
    // the message, and both halves untouched.
    let (dir, paths) = tmp_paths();
    let store = InMemoryStore::default();
    load_or_create(&paths, &store).expect("create");
    let key_before = live_key(&store);
    let big = vec![b'x'; 65 * 1024];
    std::fs::write(&paths.device_file, &big).expect("damage");

    match load_or_create(&paths, &store) {
        Err(DeviceIdentityError::Io(message)) => {
            assert!(
                message.contains("cap"),
                "the refusal names the cap: {message}"
            );
        }
        Err(other) => panic!("an oversize file is an io refusal, got {other:?}"),
        Ok(_) => panic!("an oversize file must not heal"),
    }
    assert_eq!(live_key(&store), key_before, "the key is untouched");
    assert_eq!(
        std::fs::read(&paths.device_file).expect("bytes"),
        big,
        "the file is untouched, with no sidecar"
    );
    assert!(sidecars(&dir).is_empty(), "nothing moved aside");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_invalid_device_file_names_the_recovery() {
    // Parseable but wrong: refused with the evidence intact, and a recovery
    // the user can act on — deleting the file heals either way.
    let (dir, paths) = tmp_paths();
    let store = InMemoryStore::default();
    load_or_create(&paths, &store).expect("create");
    let key_before = live_key(&store);
    let damaged = br#"{"deviceId": "not-a-uuid", "createdAt": 1, "displayName": "host", "publicKey": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=", "keyFingerprint": "00"}"#;
    std::fs::write(&paths.device_file, damaged).expect("damage");

    match load_or_create(&paths, &store) {
        Err(DeviceIdentityError::DeviceFile(message)) => {
            assert!(
                message.contains("not a UUID"),
                "the diagnosis survives: {message}"
            );
            assert!(
                message.contains("delete the file"),
                "the recovery is named: {message}"
            );
        }
        Err(other) => panic!("a validation refusal, got {other:?}"),
        Ok(_) => panic!("an invalid file must not load"),
    }
    assert_eq!(live_key(&store), key_before, "the key is untouched");
    assert_eq!(
        std::fs::read(&paths.device_file).expect("bytes"),
        damaged,
        "the file is kept"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_derived_public_key_matches_the_generated_one() {
    // snow's own keypair is the oracle: `create` generates, the derivation
    // re-derives from the stored envelope, and the two must agree.
    let (dir, paths) = tmp_paths();
    let store = InMemoryStore::default();
    let created = load_or_create(&paths, &store).expect("create");
    let private = decode_envelope(&live_key(&store)).expect("envelope");
    assert_eq!(derive_public_key(&private), created.public_key);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn move_aside_keeps_the_bytes_under_a_timestamp_name() {
    let (dir, paths) = tmp_paths();
    let store = InMemoryStore::default();
    load_or_create(&paths, &store).expect("create");
    let damaged = b"{ this is not json";
    std::fs::write(&paths.device_file, damaged).expect("damage");

    let target = move_aside(&paths);
    assert!(
        target
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("device.json.unreadable-")),
        "the sidecar is timestamped: {}",
        target.display()
    );
    assert!(!paths.device_file.exists(), "the original is gone");
    assert_eq!(std::fs::read(&target).expect("bytes"), damaged);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_salvage_never_blocks_the_heal() {
    // A directory squatting on the sidecar name defeats both the rename and
    // the copy. The salvage returns anyway with the original intact — and the
    // heal composes straight after it.
    let (dir, paths) = tmp_paths();
    let store = InMemoryStore::default();
    let created = load_or_create(&paths, &store).expect("create");
    let squat = paths.dir.join("device.json.unreadable-squat");
    std::fs::create_dir(&squat).expect("squat");

    salvage(&paths.device_file, &squat);
    assert!(
        paths.device_file.is_file(),
        "the original is untouched by a failed salvage"
    );
    assert!(squat.is_dir(), "no file was forced in");

    let rebuilt = rebuild_identity(&paths, &store, None, "the identity file is missing")
        .expect("the heal proceeds without a salvage copy");
    assert_eq!(rebuilt.public_key, created.public_key, "same key");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A credential store that fails every read, so the file diagnosis must stand
/// on its own: the error names the file state and the store failure together.
struct FailingStore;

impl SecretStore for FailingStore {
    fn get(&self, _name: &str) -> Result<Option<Vec<u8>>, SecretStoreError> {
        Err(SecretStoreError::Unavailable("keyring offline".to_string()))
    }

    fn set(&self, _name: &str, _bytes: &[u8]) -> Result<(), SecretStoreError> {
        Err(SecretStoreError::Unavailable("keyring offline".to_string()))
    }
}

#[test]
fn a_blank_file_with_an_unreadable_store_names_both_facts() {
    let (dir, paths) = tmp_paths();
    std::fs::write(&paths.device_file, b"  \n ").expect("truncate");
    let store = FailingStore;

    // Asserted on the rendered line, not the inner string: exactly one
    // "device.json" per sentence, with both facts named.
    match load_or_create(&paths, &store) {
        Err(error) => {
            let rendered = error.to_string();
            assert_eq!(
                rendered.matches("device.json").count(),
                1,
                "one device.json per sentence: {rendered}"
            );
            assert!(
                rendered.contains("holds no readable bytes")
                    && rendered.contains("keyring offline"),
                "both facts are named: {rendered}"
            );
        }
        Ok(_) => panic!("an unreadable store must not heal"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_file_with_an_unreadable_store_names_both_facts() {
    let (dir, paths) = tmp_paths();
    let store = FailingStore;
    assert!(
        !paths.device_file.exists(),
        "a genuine first run would mint here"
    );

    match load_or_create(&paths, &store) {
        Err(error) => {
            let rendered = error.to_string();
            assert_eq!(
                rendered.matches("device.json").count(),
                1,
                "one device.json per sentence: {rendered}"
            );
            assert!(
                rendered.contains("missing") && rendered.contains("keyring offline"),
                "both facts are named: {rendered}"
            );
        }
        Ok(_) => panic!("an unreadable store must not mint"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}
