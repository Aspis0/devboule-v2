//! Tests for the identity record, the key codec, the load/create lifecycle,
//! and the display-name and redaction policy.

use std::path::PathBuf;

use base64::Engine;

use super::device_identity_policy::FALLBACK_DISPLAY_NAME;
use super::{
    decode_envelope, display_name_or_fallback, encode_envelope, key_fingerprint, load_or_create,
    redact, validate_device_file, validate_display_name, DeviceFile, DeviceIdentityError,
    RemoteState, ENVELOPE_LEN, ENVELOPE_VERSION, MAX_DISPLAY_NAME_CHARS, NOISE_STATIC_SECRET_NAME,
    STATIC_KEY_LEN,
};
use crate::paths::RuntimePaths;
use crate::secret_store::{InMemoryStore, SecretStore};

fn tmp_paths() -> (PathBuf, RuntimePaths) {
    let dir = crate::test_dirs::test_temp_dir("devboule identity");
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
fn a_bom_prefixed_device_file_loads_the_same_identity() {
    let (dir, paths) = tmp_paths();
    let store = InMemoryStore::default();
    let created = load_or_create(&paths, &store).expect("create");

    let mut bytes = b"\xef\xbb\xbf".to_vec();
    bytes.extend(std::fs::read(&paths.device_file).expect("device.json"));
    std::fs::write(&paths.device_file, &bytes).expect("reseed with BOM");

    let loaded = load_or_create(&paths, &store).expect("BOM loads");
    assert_eq!(loaded.device_id, created.device_id);
    assert_eq!(loaded.public_key, created.public_key);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_blank_device_file_without_a_stored_key_names_only_the_file() {
    // Nobody stored a secret and nobody could have paired: the recovery
    // must not mention either.
    for (tag, bytes) in [
        ("empty", b"".as_slice()),
        ("whitespace", b"  \n ".as_slice()),
        ("bom-only", b"\xef\xbb\xbf".as_slice()),
    ] {
        let dir = crate::test_dirs::test_temp_dir(&format!("devboule-identity-blank-{tag}"));
        let paths = RuntimePaths::from_dir(&dir);
        let store = InMemoryStore::default();
        std::fs::write(&paths.device_file, bytes).expect("truncate");

        let error = match load_or_create(&paths, &store) {
            Err(error) => error,
            Ok(_) => panic!("{tag}: refuses"),
        };
        let rendered = error.to_string();
        assert_eq!(
            rendered.matches("device.json").count(),
            1,
            "{tag}: one device.json per sentence: {rendered}"
        );
        assert!(
            !rendered.contains("secret") && !rendered.contains("pair"),
            "{tag}: no secret and no pairings exist, so neither is named: {rendered}"
        );
        assert!(
            rendered.contains("delete the file"),
            "{tag}: the recovery is deleting the file: {rendered}"
        );
        assert!(
            store.get(NOISE_STATIC_SECRET_NAME).expect("read").is_none(),
            "{tag}: no key is minted on refusal"
        );
        assert_eq!(
            std::fs::read(&paths.device_file).expect("bytes"),
            bytes,
            "{tag}: the file is kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn an_unparseable_device_file_without_a_stored_key_stays_a_parse_error() {
    // Nothing to rebuild from: the file is all there is, so it stays a
    // parse error with the file left where it is.
    let (dir, paths) = tmp_paths();
    let store = InMemoryStore::default();
    std::fs::write(&paths.device_file, b"{ this is not json").expect("damage");

    match load_or_create(&paths, &store) {
        Err(DeviceIdentityError::Json(message)) => {
            assert!(
                message.contains("delete the file"),
                "a bare parser message helps nobody; the recovery is named: {message}"
            );
        }
        Err(other) => panic!("a parse error, got {other:?}"),
        Ok(_) => panic!("an unparseable file with no key must not load"),
    }
    assert_eq!(
        std::fs::read(&paths.device_file).expect("bytes"),
        b"{ this is not json",
        "the file is kept"
    );
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
        serde_json::to_value(RemoteState::Disabled("no tailscale".into()).to_wire()).expect("json")
            ["state"],
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
fn a_missing_key_names_the_way_out_and_its_cost() {
    // The sibling `Envelope` variant tells the person what to do; the
    // refusal to mint a key must do the same, because a Retry button on
    // this sentence fails identically forever.
    let rendered = DeviceIdentityError::KeyMissing.to_string();
    assert!(
        rendered.contains("device.json") && rendered.contains("secret"),
        "the message must name both halves of the state: {rendered}"
    );
    assert!(
        rendered.contains("pair"),
        "the message must say the pairings have to be made again: {rendered}"
    );
}

#[test]
fn a_malformed_envelope_is_not_reported_as_key_missing() {
    // A truncated or version-shifted secret must fail loudly with the
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
#[test]
fn display_name_accepts_ordinary_hostnames() {
    for good in [
        "Marcolenovo",
        "macbook-pro",
        "TABLET-V477JRIG",
        "host name",            // a single interior space is readable
        "café",                 // a legitimate non-ASCII name
        "host.tailnet.ts.net.", // the LocalAPI spelling, dots included
    ] {
        assert!(
            validate_display_name(good).is_ok(),
            "{good:?} must be accepted: {:?}",
            validate_display_name(good)
        );
    }
    // Exactly at the bound.
    let at_bound = "x".repeat(MAX_DISPLAY_NAME_CHARS);
    assert!(validate_display_name(&at_bound).is_ok());
    assert!(validate_display_name(&format!("{at_bound}x")).is_err());
}

#[test]
fn display_name_rejects_what_must_not_be_shown() {
    for (bad, why) in [
        ("", "empty"),
        ("   ", "starts or ends with whitespace"),
        // The message names both ends at once, so these two expect the same
        // text: the check is `name.trim() != name`.
        (" leading", "starts or ends with whitespace"),
        ("trailing ", "starts or ends with whitespace"),
        ("two\nlines", "a control character"),
        ("tab\there", "a control character"),
        ("bell\u{7}", "a control character"),
        ("separator\u{2028}line", "a line break"),
        ("paragraph\u{2029}break", "a line break"),
        ("soft\u{ad}hyphen", "an invisible formatting character"),
        ("zero\u{200b}width", "an invisible formatting character"),
        ("join\u{200d}er", "an invisible formatting character"),
        (
            "right-to-left\u{202e}override",
            "an invisible formatting character",
        ),
        (
            "pop\u{202c}directional",
            "an invisible formatting character",
        ),
        (
            "byte-order\u{feff}mark",
            "an invisible formatting character",
        ),
    ] {
        // `err()`, not `unwrap_or_else`: the helper returns
        // `Result<(), String>`, so the success arm is `()`, and this test
        // wants the message.
        let error = validate_display_name(bad)
            .err()
            .unwrap_or_else(|| panic!("{bad:?} ({why}) must be refused"));
        assert!(
            error.contains(why),
            "the reason for {bad:?} must name the problem ({why}), got {error:?}"
        );
    }
}

#[test]
fn a_name_that_cannot_be_shown_falls_back_rather_than_failing() {
    // Our own name comes from the operating system, so an unusable hostname
    // must not stop the daemon from having an identity; it is replaced.
    assert_eq!(display_name_or_fallback("Marcolenovo"), "Marcolenovo");
    for bad in [
        "",
        "  ",
        "bad\nname",
        "x".repeat(MAX_DISPLAY_NAME_CHARS + 1).as_str(),
    ] {
        assert_eq!(
            display_name_or_fallback(bad),
            FALLBACK_DISPLAY_NAME,
            "{bad:?} must fall back"
        );
    }
}

/// The name this daemon publishes always passes the check the far side
/// applies to it, including when the machine's hostname is unusable.
#[test]
fn our_own_published_name_always_validates() {
    let (dir, paths) = tmp_paths();
    let store = InMemoryStore::default();
    let identity = load_or_create(&paths, &store).expect("identity");
    validate_display_name(&identity.display_name)
        .expect("the name this daemon publishes must pass the peer's check");

    // And a hand-edited device.json cannot smuggle one past `load`.
    let mut file: DeviceFile =
        serde_json::from_slice(&std::fs::read(&paths.device_file).expect("device.json"))
            .expect("json");
    file.display_name = "invisible\u{202e}name".to_string();
    crate::atomic::atomic_write(
        &paths.device_file,
        &serde_json::to_vec_pretty(&file).expect("json"),
    )
    .expect("write back");
    let reloaded = load_or_create(&paths, &store).expect("reload");
    validate_display_name(&reloaded.display_name)
        .expect("a hand-edited name must be sanitised on load");
    assert_eq!(reloaded.display_name, FALLBACK_DISPLAY_NAME);

    let _ = std::fs::remove_dir_all(&dir);
}
