//! Tests for the wire messages: serialization, defaults and the envelope shape.

use super::*;
use super::{
    VocabularyFeature, VocabularyFeatureControl, VocabularyFeatureOption, VocabularyFeatures,
};
use crate::{SessionState, SessionStateSnapshot, UnattendedState};

#[test]
fn the_pairing_code_is_never_debug_formatted() {
    let complete = ClientMessage::PairingComplete {
        id: 1,
        address: "100.64.0.2:47831".to_string(),
        code: PairingSecret::new("ABCD2345"),
        role: PeerRole::Client,
    };
    let rendered = format!("{complete:?}");
    assert!(
        !rendered.contains("ABCD2345"),
        "a pairing code must never reach a log or an error string: {rendered}"
    );
    assert!(rendered.contains("<redacted>"), "{rendered}");

    let code = DaemonMessage::PairingCode {
        id: 1,
        code: PairingSecret::new("ABCD2345"),
        expires_at: 1,
        address: "100.64.0.2:47831".to_string(),
    };
    let rendered = format!("{code:?}");
    assert!(!rendered.contains("ABCD2345"), "{rendered}");
    assert!(rendered.contains("<redacted>"), "{rendered}");

    // The ordinary case: a generic error path formats the whole frame.
    let wrapped = format!("unexpected daemon frame {code:?}");
    assert!(!wrapped.contains("ABCD2345"), "{wrapped}");
}

#[test]
fn the_three_vocabulary_states_are_three_distinct_wire_words() {
    // `present`, `none` and `absent` are three different facts and three
    // different wire values. Asserting on the serialised JSON, not the
    // Rust variant, is what makes a collapse of two of them — into an
    // empty `present`, a shared "nothing", or any synonym — a red test
    // rather than a silent loss.
    let words = [
        (VocabularyState::Present, "present"),
        (VocabularyState::None, "none"),
        (VocabularyState::Absent, "absent"),
    ];
    for (state, word) in words {
        assert_eq!(
            serde_json::to_value(state).expect("json"),
            serde_json::json!(word),
            "{word} must serialise to exactly its wire word"
        );
    }
}

#[test]
fn the_vocabulary_origins_and_source_keep_their_wire_words() {
    assert_eq!(
        serde_json::to_value(VocabularyOrigin::Provider).expect("json"),
        serde_json::json!("provider")
    );
    assert_eq!(
        serde_json::to_value(VocabularyOrigin::Daemon).expect("json"),
        serde_json::json!("daemon")
    );
    assert_eq!(
        serde_json::to_value(VocabularySource::Cache).expect("json"),
        serde_json::json!("cache")
    );
    assert_eq!(
        serde_json::to_value(VocabularySource::Probe).expect("json"),
        serde_json::json!("probe")
    );
}

#[test]
fn an_unset_origin_is_absent_from_the_wire_not_null() {
    // One shape goes out: an optional reply field is omitted, never an
    // explicit `null`. The TypeScript reader tolerates both because a
    // reader should be tolerant; that tolerance is not a second encoding
    // the daemon may pick.
    let axis = VocabularyModels {
        state: VocabularyState::Absent,
        origin: None,
        items: Vec::new(),
    };
    let json = serde_json::to_value(&axis).expect("json");
    assert!(json.get("origin").is_none(), "got {json}");
    assert!(json.get("items").is_some(), "got {json}");

    // The set case still emits the key, so the omission is a decision and
    // not a lost field.
    let axis = VocabularyModels {
        state: VocabularyState::Present,
        origin: Some(VocabularyOrigin::Provider),
        items: Vec::new(),
    };
    let json = serde_json::to_value(&axis).expect("json");
    assert_eq!(json.get("origin"), Some(&serde_json::json!("provider")));
}

#[test]
fn the_vocabulary_reply_omits_probed_at_on_a_probe_reply_and_names_the_field_camel_case() {
    // `probedAtMs` is a cache fact: a probe reply is fresh by definition,
    // so the field is absent — not null. The cached case emits the camelCase
    // key the TypeScript mirror reads.
    let probe = DaemonMessage::ProviderVocabulary {
        id: 7,
        provider: "claude".to_string(),
        models: VocabularyModels {
            state: VocabularyState::Present,
            origin: Some(VocabularyOrigin::Provider),
            items: Vec::new(),
        },
        modes: VocabularyModes {
            state: VocabularyState::Present,
            origin: Some(VocabularyOrigin::Daemon),
            items: Vec::new(),
        },
        features: None,
        source: VocabularySource::Probe,
        probed_at_ms: None,
    };
    let json = serde_json::to_value(&probe).expect("json");
    assert!(json.get("probedAtMs").is_none(), "got {json}");
    assert_eq!(json.get("source"), Some(&serde_json::json!("probe")));
    assert_eq!(json.get("provider"), Some(&serde_json::json!("claude")));

    let cached = DaemonMessage::ProviderVocabulary {
        id: 8,
        provider: "claude".to_string(),
        models: VocabularyModels {
            state: VocabularyState::Present,
            origin: Some(VocabularyOrigin::Provider),
            items: Vec::new(),
        },
        modes: VocabularyModes {
            state: VocabularyState::Absent,
            origin: None,
            items: Vec::new(),
        },
        // The features axis the daemon answers with today: a present list
        // whose authorship sits on the row, because one provider's list mixes
        // the agent's declarations with this daemon's own tick.
        features: Some(
            VocabularyFeatures::new(
                VocabularyState::Present,
                vec![VocabularyFeature {
                    id: "autoAccept".to_string(),
                    label: "Auto accept".to_string(),
                    author: VocabularyOrigin::Daemon,
                    control: VocabularyFeatureControl::Toggle,
                    options: Vec::new(),
                    models: None,
                }],
            )
            .expect("a present axis carries its items"),
        ),
        source: VocabularySource::Cache,
        probed_at_ms: Some(1_700_000_000_000),
    };
    let json = serde_json::to_value(&cached).expect("json");
    assert_eq!(
        json.get("probedAtMs"),
        Some(&serde_json::json!(1_700_000_000_000_u64)),
        "got {json}"
    );
    // The field's own wire shape: camelCase key, the discriminant spelled
    // `type` as Paseo's feature union spells it, and no `options` array on a
    // toggle — a form that read `options: []` and a form that read `undefined`
    // would draw two different widgets for one row.
    let feature = &json["features"]["items"][0];
    assert_eq!(feature.get("type"), Some(&serde_json::json!("toggle")));
    assert_eq!(feature.get("author"), Some(&serde_json::json!("daemon")));
    assert!(feature.get("options").is_none(), "got {feature}");
    assert!(feature.get("models").is_none(), "got {feature}");
    // `probing` is omitted when false for the same reason `options` is: an
    // older reader must not see a key it has no meaning for, and a reader that
    // sees the key knows the read is running.
    assert!(json["features"].get("probing").is_none());
}

/// The two states a form cannot afford to confuse, on the wire: a provider
/// that was read and offered nothing (`none`, `probing` omitted) against a
/// read still running (`absent` with `probing: true`). Collapsing them would
/// let a cold start render as "this provider has no features", which is a
/// claim about the provider the daemon has not earned.
#[test]
fn a_running_read_is_not_an_answer_of_no_features() {
    let read = serde_json::to_value(
        VocabularyFeatures::new(VocabularyState::None, Vec::new()).expect("an answered axis"),
    )
    .expect("json");
    assert_eq!(read.get("state"), Some(&serde_json::json!("none")));
    assert!(read.get("probing").is_none(), "got {read}");

    let running = serde_json::to_value(VocabularyFeatures::probing()).expect("json");
    assert_eq!(running.get("state"), Some(&serde_json::json!("absent")));
    assert_eq!(running.get("probing"), Some(&serde_json::json!(true)));

    // A `present` axis with nothing in it is a collapsed absence, and the
    // constructor that every builder uses refuses it — as it does for the
    // models and modes axes.
    assert!(VocabularyFeatures::new(VocabularyState::Present, Vec::new()).is_err());
    assert!(VocabularyFeatures::new(VocabularyState::Absent, Vec::new()).is_ok());
}

/// The declaration's own invariant: the control and the option list cannot
/// disagree, because each is the reason the other exists. And the model gate
/// is read, not stored: `None` means every model, so the row is offered on a
/// model the daemon has never heard of, and `Some` means exactly the ids it
/// lists.
#[test]
fn a_declaration_cannot_pair_a_toggle_with_choices_or_a_select_with_none() {
    let choices = vec![VocabularyFeatureOption {
        id: "on".to_string(),
        label: "On".to_string(),
    }];
    assert!(VocabularyFeature::new(
        "fast".to_string(),
        "Fast".to_string(),
        VocabularyOrigin::Provider,
        VocabularyFeatureControl::Toggle,
        choices.clone(),
    )
    .is_err());
    assert!(VocabularyFeature::new(
        "engine".to_string(),
        "Engine".to_string(),
        VocabularyOrigin::Provider,
        VocabularyFeatureControl::Select,
        Vec::new(),
    )
    .is_err());
    let select = VocabularyFeature::new(
        "engine".to_string(),
        "Engine".to_string(),
        VocabularyOrigin::Provider,
        VocabularyFeatureControl::Select,
        choices,
    )
    .expect("a select with a choice");
    assert!(select.offered_on(None), "no gate means every model");

    let gated = VocabularyFeature {
        models: Some(vec!["claude-opus-5".to_string()]),
        ..select.clone()
    };
    assert!(gated.offered_on(Some("claude-opus-5")));
    assert!(!gated.offered_on(Some("claude-sonnet-5")));
    // "No model chosen yet" is not "a model that carries it", and the row a
    // human cannot have earned stays off the form until they name one.
    assert!(!gated.offered_on(None));
}

#[test]
fn origin_on_the_wire_is_present_exactly_when_the_state_is_present() {
    // The biconditional, asserted in both directions. On the wire: a
    // `present` state implies an `origin` key, and a `none`/`absent`
    // state implies no `origin` key — so a reader can never hold an
    // origin without a present list, and the key can never silently
    // become a second source of truth about the state. In the
    // constructor: the illegal pairs themselves — `(none, Some)`,
    // `(absent, Some)`, and a `present` without an origin — are built
    // through the constructor every axis builder uses and must be
    // refused, because a legal-inputs-only wire test exercises
    // `skip_serializing_if`, not the invariant.
    fn axis_json(state: VocabularyState, origin: Option<VocabularyOrigin>) -> serde_json::Value {
        serde_json::to_value(VocabularyModes {
            state,
            origin,
            items: Vec::new(),
        })
        .expect("json")
    }
    let json = axis_json(VocabularyState::Present, Some(VocabularyOrigin::Daemon));
    assert!(
        json.get("origin").is_some(),
        "a present axis must carry its origin, got {json}"
    );
    for state in [VocabularyState::None, VocabularyState::Absent] {
        let json = axis_json(state, None);
        assert!(
            json.get("origin").is_none(),
            "a {state:?} axis must not carry an origin, got {json}"
        );
    }
    // The forbidden half of the biconditional, on both axis types: each
    // illegal pair goes through the real constructor and is refused.
    for (state, origin) in [
        (VocabularyState::None, Some(VocabularyOrigin::Daemon)),
        (VocabularyState::Absent, Some(VocabularyOrigin::Provider)),
        (VocabularyState::Present, None),
    ] {
        assert!(
            VocabularyModes::new(state, origin, Vec::new()).is_err(),
            "an axis of {state:?} with origin {origin:?} must be refused"
        );
        assert!(
            VocabularyModels::new(state, origin, Vec::new()).is_err(),
            "an axis of {state:?} with origin {origin:?} must be refused"
        );
    }
    // Every legal pair still builds, in both directions of the
    // biconditional.
    assert!(VocabularyModes::new(
        VocabularyState::Present,
        Some(VocabularyOrigin::Provider),
        Vec::new()
    )
    .is_ok());
    assert!(VocabularyModels::new(
        VocabularyState::Present,
        Some(VocabularyOrigin::Daemon),
        Vec::new()
    )
    .is_ok());
    for state in [VocabularyState::None, VocabularyState::Absent] {
        assert!(VocabularyModes::new(state, None, Vec::new()).is_ok());
        assert!(VocabularyModels::new(state, None, Vec::new()).is_ok());
    }
}

#[test]
fn the_devices_wire_contract_round_trips_with_its_exact_field_names() {
    // Brief 1b's wire contract is normative for the frontend, so the field
    // names are asserted on the serialised JSON, not on the struct.
    let pending = PendingPairing {
        device_id: "dev-2".to_string(),
        display_name: "Phone".to_string(),
        role: PeerRole::Client,
        key_fingerprint: "ab".repeat(16),
        address: "100.64.0.2:47831".to_string(),
        expires_at: 1_700_000_000_000,
    };
    let pending_json = serde_json::to_value(&pending).expect("json");
    for key in [
        "deviceId",
        "displayName",
        "role",
        "keyFingerprint",
        "address",
        "expiresAt",
    ] {
        assert!(
            pending_json.get(key).is_some(),
            "PendingPairing is missing {key}"
        );
    }
    assert_eq!(pending_json["role"], "client");
    assert_eq!(
        serde_json::from_value::<PendingPairing>(pending_json.clone()).expect("back"),
        pending
    );

    let row = PeerRow {
        device_id: "dev-1".to_string(),
        display_name: "MacBook".to_string(),
        role: PeerRole::Daemon,
        public_key: "AAAA".to_string(),
        key_fingerprint: "cd".repeat(16),
        binding_kind: "tailnet".to_string(),
        binding_node_name: Some("host.tailnet.ts.net.".to_string()),
        binding_login_name: Some("user@example.com".to_string()),
        address: "100.64.0.1:47831".to_string(),
        paired_at: 1_700_000_000_000,
        revoked_at: None,
        caps: vec!["view".to_string()],
        paired_by_user: Some("S-1-5-21-1".to_string()),
        online: true,
    };
    let row_json = serde_json::to_value(&row).expect("json");
    for key in [
        "deviceId",
        "displayName",
        "role",
        "publicKey",
        "keyFingerprint",
        "bindingKind",
        "bindingNodeName",
        "bindingLoginName",
        "address",
        "pairedAt",
        "revokedAt",
        "caps",
        "pairedByUser",
        "online",
    ] {
        assert!(row_json.get(key).is_some(), "PeerRow is missing {key}");
    }
    // Present as `null`, not absent: the panel distinguishes the two.
    assert!(row_json["revokedAt"].is_null());
    assert_eq!(row_json["role"], "daemon");
    assert_eq!(
        serde_json::from_value::<PeerRow>(row_json.clone()).expect("back"),
        row
    );

    let self_info = SelfInfo {
        device_id: "dev-1".to_string(),
        display_name: "MacBook".to_string(),
        public_key: "AAAA".to_string(),
        key_fingerprint: "cd".repeat(16),
        addresses: vec!["100.64.0.1".to_string()],
        port: 47831,
        daemon_version: "0.1.0".to_string(),
        protocol_version: crate::PROTOCOL_VERSION,
        remote: Some(RemoteState::enabled()),
    };
    let self_json = serde_json::to_value(&self_info).expect("json");
    for key in [
        "deviceId",
        "displayName",
        "publicKey",
        "keyFingerprint",
        "addresses",
        "port",
        "daemonVersion",
        "protocolVersion",
        "remote",
    ] {
        assert!(self_json.get(key).is_some(), "SelfInfo is missing {key}");
    }
    assert_eq!(
        self_json["remote"],
        serde_json::json!({ "state": "enabled", "reason": null })
    );

    // A projection withholds by **value**, never by key presence.
    //
    // This is the C4 fix: `addresses`, `port`, `publicKey` and
    // `keyFingerprint` are always in the frame, empty when the projection
    // has nothing to put in them, because the 1b contract types them as
    // required and TypeScript cannot check a key the daemon chose to omit.
    // The panel's identity card reads `self.addresses.length` with no
    // guard, so an omitted key was a crash in the Devices tab whenever
    // remote was off (the default first-run state).
    //
    // `remote` is the exception: it stays absent for the `Daemon`
    // projection, because `RemoteState` has no "unknown" variant, so its
    // only way to withhold the listener state is to omit the object.
    let withheld = SelfInfo {
        device_id: "dev-1".to_string(),
        display_name: "MacBook".to_string(),
        public_key: String::new(),
        key_fingerprint: String::new(),
        addresses: Vec::new(),
        port: 0,
        daemon_version: "0.1.0".to_string(),
        protocol_version: crate::PROTOCOL_VERSION,
        remote: None,
    };
    let withheld_json = serde_json::to_value(&withheld).expect("json");
    assert_eq!(
        withheld_json["addresses"],
        serde_json::json!([]),
        "an empty address list is the empty array, not an absent key: {withheld_json}"
    );
    assert_eq!(
        withheld_json["port"],
        serde_json::json!(0),
        "a listener-less self_info carries port 0: {withheld_json}"
    );
    assert_eq!(withheld_json["publicKey"], "");
    assert_eq!(withheld_json["keyFingerprint"], "");
    for key in ["publicKey", "keyFingerprint", "addresses", "port"] {
        assert!(
            withheld_json.get(key).is_some(),
            "a withheld SelfInfo must still carry {key}: {withheld_json}"
        );
    }
    assert!(
        withheld_json.get("remote").is_none(),
        "the Daemon projection withholds the listener state by omission: {withheld_json}"
    );
    for key in [
        "deviceId",
        "displayName",
        "daemonVersion",
        "protocolVersion",
    ] {
        assert!(
            withheld_json.get(key).is_some(),
            "a withheld SelfInfo keeps {key}: {withheld_json}"
        );
    }

    // The other direction, and the one the panel actually hits: a real
    // local projection with remote off serialises every contract key with
    // empty values, so `self.addresses.length` has something to read.
    let local_off = SelfInfo {
        remote: Some(RemoteState::disabled("no tailscale")),
        ..withheld.clone()
    };
    let local_json = serde_json::to_value(&local_off).expect("json");
    assert_eq!(local_json["addresses"], serde_json::json!([]));
    assert_eq!(local_json["port"], 0);
    assert_eq!(local_json["remote"]["state"], "disabled");
    assert!(
        local_json["addresses"].is_array() && local_json["port"].is_number(),
        "the panel can read these unconditionally: {local_json}"
    );

    // The reply variants carry exactly the contract's fields.
    let devices = serde_json::to_value(DaemonMessage::Devices {
        id: 4,
        self_info: self_info.clone(),
        peers: vec![row.clone()],
        pending: vec![pending.clone()],
    })
    .expect("json");
    assert_eq!(devices["type"], "devices");
    assert_eq!(devices["id"], 4);
    for key in ["selfInfo", "peers", "pending"] {
        assert!(devices.get(key).is_some(), "Devices is missing {key}");
    }
    assert_eq!(
        serde_json::to_value(DaemonMessage::PairingPending {
            id: 5,
            peer: pending.clone(),
        })
        .expect("json")["type"],
        "pairing_pending"
    );
    assert_eq!(
        serde_json::to_value(DaemonMessage::PairingDone {
            id: 6,
            peer: row.clone(),
        })
        .expect("json")["type"],
        "pairing_done"
    );
    assert_eq!(
        serde_json::to_value(DaemonMessage::PeerUpdated { id: 7, peer: row }).expect("json")
            ["type"],
        "peer_updated"
    );
    let declined = serde_json::to_value(DaemonMessage::PairingDeclined {
        id: 8,
        device_id: "dev-2".to_string(),
    })
    .expect("json");
    assert_eq!(declined["type"], "pairing_declined");
    assert_eq!(declined["id"], 8);
    assert_eq!(declined["deviceId"], "dev-2");

    // And the request variants, with their argument names.
    let requests: [(ClientMessage, &str, &[&str]); 6] = [
        (
            ClientMessage::DevicesList { id: 1 },
            "devices_list",
            &["id"],
        ),
        (
            ClientMessage::PairingStart {
                id: 1,
                role: PeerRole::Daemon,
            },
            "pairing_start",
            &["id", "role"],
        ),
        (
            ClientMessage::PairingComplete {
                id: 1,
                address: "100.64.0.2:47831".to_string(),
                code: PairingSecret::new("ABCD2345"),
                role: PeerRole::Client,
            },
            "pairing_complete",
            &["id", "address", "code", "role"],
        ),
        (
            ClientMessage::PairingConfirm {
                id: 1,
                device_id: "dev-2".to_string(),
                accept: true,
            },
            "pairing_confirm",
            &["id", "deviceId", "accept"],
        ),
        (
            ClientMessage::PeerRevoke {
                id: 1,
                device_id: "dev-2".to_string(),
            },
            "peer_revoke",
            &["id", "deviceId"],
        ),
        (
            ClientMessage::PeerSetCaps {
                id: 1,
                device_id: "dev-2".to_string(),
                caps: vec!["view".to_string(), "send".to_string()],
            },
            "peer_set_caps",
            &["id", "deviceId", "caps"],
        ),
    ];
    for (request, tag, keys) in requests {
        let json = serde_json::to_value(&request).expect("json");
        assert_eq!(json["type"], tag, "{request:?}");
        for key in keys {
            assert!(json.get(key).is_some(), "{tag} is missing {key}");
        }
        assert_eq!(
            serde_json::from_value::<ClientMessage>(json.clone()).expect("back"),
            request
        );
    }
}

#[test]
fn devices_capability_is_advertised_and_the_peer_caps_are_the_agreed_set() {
    assert!(crate::m3a_daemon_capabilities()
        .iter()
        .any(|capability| capability.as_str() == crate::caps::DEVICES));
    assert_eq!(
        PEER_CAPS,
        [
            "view",
            "send",
            "answer_permissions",
            "create_sessions",
            "roster",
            "search",
            "admin"
        ]
    );
    // Frame compatibility is the negotiated protocol capability
    // `peer_agents`, a different mechanism advertised in both hello
    // lists; it is deliberately not a peer capability name.
    assert!(crate::m3a_daemon_capabilities()
        .iter()
        .any(|capability| capability.as_str() == crate::caps::PEER_AGENTS));
    assert!(crate::m3a_client_capabilities()
        .iter()
        .any(|capability| capability.as_str() == crate::caps::PEER_AGENTS));
    assert!(!PEER_CAPS.contains(&crate::caps::PEER_AGENTS));
    assert!(crate::m3a_daemon_capabilities()
        .iter()
        .any(|capability| capability.as_str() == crate::caps::AGENT_MESSAGES));
    assert!(crate::m3a_client_capabilities()
        .iter()
        .any(|capability| capability.as_str() == crate::caps::AGENT_MESSAGES));
    assert!(!PEER_CAPS.contains(&crate::caps::AGENT_MESSAGES));
}

#[test]
fn a_new_pairing_is_born_with_every_capability() {
    // The 2026-09-21 parity decision at the protocol layer: the default grant is
    // the whole wire set, so "the phone is mine" is the state a pairing starts
    // in and a person narrows it afterwards, per device.
    assert_eq!(PEER_DEFAULT_CAPS, PEER_CAPS);
    // Spelled as well as compared, because the comparison is what a seventh
    // capability must break deliberately: the lengths differ, so whoever adds a
    // name decides whether the default follows it instead of inheriting it
    // silently.
    assert_eq!(
        PEER_DEFAULT_CAPS,
        [
            "view",
            "send",
            "answer_permissions",
            "create_sessions",
            "roster",
            "search",
            "admin"
        ]
    );
}

#[test]
fn remote_state_serialises_exactly_the_agreed_shape() {
    // Brief 1b's wire contract, pinned here because the frontend is built
    // against this JSON: `{ state, reason }`, both keys always present.
    let enabled = serde_json::to_value(RemoteState::enabled()).expect("json");
    assert_eq!(
        enabled,
        serde_json::json!({ "state": "enabled", "reason": null })
    );

    let disabled =
        serde_json::to_value(RemoteState::disabled("Tailscale is not running")).expect("json");
    assert_eq!(
        disabled,
        serde_json::json!({ "state": "disabled", "reason": "Tailscale is not running" })
    );

    let missing = serde_json::to_value(RemoteState::key_missing()).expect("json");
    assert_eq!(missing["state"], "key_missing");
    assert!(missing["reason"].is_string());

    // Round-trips, so a local client can echo it back in a test fixture.
    for state in [
        RemoteState::enabled(),
        RemoteState::disabled("why"),
        RemoteState::key_missing(),
    ] {
        let json = serde_json::to_string(&state).expect("serialize");
        assert_eq!(
            serde_json::from_str::<RemoteState>(&json).expect("deserialize"),
            state
        );
    }
}

#[test]
fn only_writes_are_state_changing_and_every_variant_names_itself() {
    assert!(!ClientMessage::Ping { id: 1 }.is_state_changing());
    assert!(!ClientMessage::Status { id: 1 }.is_state_changing());
    assert!(!ClientMessage::SessionsList { id: 1 }.is_state_changing());
    assert!(!ClientMessage::JournalUsage { id: 1 }.is_state_changing());
    assert!(!ClientMessage::ProjectsList { id: 1 }.is_state_changing());
    assert!(!ClientMessage::JournalRetentionGet { id: 1 }.is_state_changing());
    assert!(ClientMessage::Shutdown { id: 1 }.is_state_changing());
    assert!(ClientMessage::JournalRetentionSet {
        id: 1,
        max_age_ms: None,
        max_bytes: None,
        max_sessions: None,
        session_max_bytes: None,
        idempotency_key: None,
    }
    .is_state_changing());
    assert!(ClientMessage::SessionSend {
        id: 1,
        session_id: "s.a.1".to_string(),
        subscription_id: 1,
        text: "hi".to_string(),
        attachments: Vec::new(),
        active_turn_behavior: None,
        attachment_references: Vec::new(),
        idempotency_key: None,
    }
    .is_state_changing());
    assert!(ClientMessage::ProvidersRefresh { id: 1 }.is_state_changing());
    assert!(!ClientMessage::ToolPolicyGet { id: 1 }.is_state_changing());
    assert!(ClientMessage::ToolPolicySet {
        id: 1,
        provider_id: "claude".to_string(),
        enabled: None,
        disabled_tools: Vec::new(),
    }
    .is_state_changing());
    assert_eq!(
        ClientMessage::ToolPolicyGet { id: 1 }.name(),
        "ToolPolicyGet"
    );
    assert_eq!(
        ClientMessage::ToolPolicySet {
            id: 1,
            provider_id: "claude".to_string(),
            enabled: Some(true),
            disabled_tools: Vec::new(),
        }
        .name(),
        "ToolPolicySet"
    );

    assert_eq!(ClientMessage::Ping { id: 1 }.name(), "Ping");
    assert_eq!(
        ClientMessage::SessionSetMode {
            id: 1,
            session_id: "s.a.1".to_string(),
            mode_id: "acceptEdits".to_string(),
        }
        .name(),
        "SessionSetMode"
    );
    assert_eq!(
        ClientMessage::Hello(crate::ClientHello::m3a(
            crate::OwnerId::new("u", "c").expect("owner"),
            "test",
        ))
        .name(),
        "Hello"
    );
    assert!(!ClientMessage::Hello(crate::ClientHello::m3a(
        crate::OwnerId::new("u", "c").expect("owner"),
        "test",
    ))
    .is_state_changing());
}

#[test]
fn detach_close_stop_are_three_type_tags() {
    let detach = serde_json::to_value(ClientMessage::SessionDetach {
        id: 1,
        session_id: "s.a.1".to_string(),
        subscription_id: 11,
    })
    .expect("json");
    let close = serde_json::to_value(ClientMessage::SessionClose {
        id: 1,
        session_id: "s.a.1".to_string(),
        idempotency_key: None,
    })
    .expect("json");
    let stop = serde_json::to_value(ClientMessage::SessionStop {
        id: 1,
        session_id: "s.a.1".to_string(),
        subscription_id: 11,
    })
    .expect("json");
    assert_eq!(detach["type"], "session_detach");
    assert_eq!(close["type"], "session_close");
    assert_eq!(stop["type"], "session_stop");
    assert_ne!(detach["type"], close["type"]);
    assert_ne!(close["type"], stop["type"]);
    assert_ne!(detach["type"], stop["type"]);
}

#[test]
fn session_send_without_attachments_still_deserializes() {
    // An older client does not know the field at all. Dropping it here
    // would make `serde(default)` on the variant look like it worked while
    // every other builder in this crate still had to pass it: the frame
    // below is the one a v4 client sends today.
    let frame =
        r#"{"type":"session_send","id":7,"sessionId":"s.a.1","subscriptionId":11,"text":"hello"}"#;
    let message: ClientMessage = serde_json::from_str(frame).expect("old frame");
    assert_eq!(
        message,
        ClientMessage::SessionSend {
            id: 7,
            session_id: "s.a.1".to_string(),
            subscription_id: 11,
            text: "hello".to_string(),
            attachments: Vec::new(),
            active_turn_behavior: None,
            attachment_references: Vec::new(),
            idempotency_key: None,
        }
    );
}

#[test]
fn session_send_with_attachments_round_trips() {
    let message = ClientMessage::SessionSend {
        id: 7,
        session_id: "s.a.1".to_string(),
        subscription_id: 11,
        text: "hello".to_string(),
        attachments: vec![PromptAttachment {
            name: "photo.png".to_string(),
            mime_type: "image/png".to_string(),
            data: "AA==".to_string(),
        }],
        active_turn_behavior: None,
        attachment_references: Vec::new(),
        idempotency_key: None,
    };
    let value = serde_json::to_value(&message).expect("json");
    assert_eq!(value["attachments"][0]["mimeType"], "image/png");
    assert_eq!(value["attachments"][0]["name"], "photo.png");
    assert_eq!(value["attachments"][0]["data"], "AA==");
    let decoded: ClientMessage = serde_json::from_value(value).expect("round trip");
    assert_eq!(decoded, message);
}

#[test]
fn session_send_accepts_only_the_steer_active_turn_behavior() {
    let steer: ClientMessage = serde_json::from_str(
            r#"{"type":"session_send","id":7,"sessionId":"s.a.1","subscriptionId":11,"text":"hello","activeTurnBehavior":"steer"}"#,
        )
        .expect("steer frame");
    assert!(matches!(
        steer,
        ClientMessage::SessionSend {
            active_turn_behavior: Some(ActiveTurnBehavior::Steer),
            ..
        }
    ));
    assert!(serde_json::from_str::<ClientMessage>(
            r#"{"type":"session_send","id":7,"sessionId":"s.a.1","subscriptionId":11,"text":"hello","activeTurnBehavior":"replace"}"#
        )
        .is_err());
    // The two shapes a hand-written frame gets wrong: an empty value (the
    // field is present, so `default` does not apply) and a differently-cased
    // spelling of the one behaviour. Both must be refused by the decoder,
    // which is where the daemon's own frame reader refuses them: a steer the
    // daemon read as "the default" would be a plain send the caller never asked
    // for.
    assert!(serde_json::from_str::<ClientMessage>(
            r#"{"type":"session_send","id":7,"sessionId":"s.a.1","subscriptionId":11,"text":"hello","activeTurnBehavior":""}"#
        )
        .is_err());
    assert!(serde_json::from_str::<ClientMessage>(
            r#"{"type":"session_send","id":7,"sessionId":"s.a.1","subscriptionId":11,"text":"hello","activeTurnBehavior":"Steer"}"#
        )
        .is_err());
}

#[test]
fn agent_message_receipt_round_trips_with_the_wire_state() {
    let message = DaemonMessage::AgentMessageReceipt {
        id: 9,
        state: AgentMessageState::RejectedAbsent,
    };
    let value = serde_json::to_value(&message).expect("json");
    assert_eq!(value["type"], "agent_message_receipt");
    assert_eq!(value["state"], "rejected_absent");
    assert_eq!(
        serde_json::from_value::<DaemonMessage>(value).expect("decode"),
        message
    );
}

/// A2-07: the receipt that says a caller was *denied* has its own wire
/// spelling, and it is not the one that blames the pairing.
#[test]
fn a_denied_agent_message_has_its_own_wire_state() {
    let message = DaemonMessage::AgentMessageReceipt {
        id: 10,
        state: AgentMessageState::RejectedDenied,
    };
    let value = serde_json::to_value(&message).expect("json");
    assert_eq!(value["state"], "rejected_denied");
    assert_eq!(
        serde_json::from_value::<DaemonMessage>(value).expect("decode"),
        message
    );
    assert_ne!(
        serde_json::to_value(AgentMessageState::RejectedUnpaired).expect("json"),
        serde_json::to_value(AgentMessageState::RejectedDenied).expect("json"),
        "a denial is not an unpaired caller, and the wire must not say it is"
    );
}

#[test]
fn session_send_with_no_attachments_omits_the_field() {
    let value = serde_json::to_value(ClientMessage::SessionSend {
        id: 7,
        session_id: "s.a.1".to_string(),
        subscription_id: 11,
        text: "hello".to_string(),
        attachments: Vec::new(),
        active_turn_behavior: None,
        attachment_references: Vec::new(),
        idempotency_key: None,
    })
    .expect("json");
    assert!(
        value.get("attachments").is_none(),
        "an empty list must not add a field to every send frame"
    );
}

#[test]
fn session_deposit_names_itself_and_is_state_changing() {
    // A deposit writes a file under a session, so it sits on the audit side
    // of `is_state_changing` and names itself in the audit rows. A design
    // that quietly made it read-only to avoid the audit line would be
    // wrong, and this is where that decision is pinned.
    let deposit = ClientMessage::SessionDeposit {
        id: 4,
        session_id: "s.a.1".to_string(),
        attachment: PromptAttachment {
            name: "page-1.jpg".to_string(),
            mime_type: "image/jpeg".to_string(),
            data: "AA==".to_string(),
        },
    };
    assert_eq!(deposit.name(), "SessionDeposit");
    assert!(deposit.is_state_changing());
    assert_eq!(deposit.request_id(), Some(4));
    assert_eq!(deposit.idempotency_key(), None);
}

#[test]
fn session_deposit_round_trips_with_a_camel_case_envelope() {
    let message = ClientMessage::SessionDeposit {
        id: 4,
        session_id: "s.a.1".to_string(),
        attachment: PromptAttachment {
            name: "page-1.jpg".to_string(),
            mime_type: "image/jpeg".to_string(),
            data: "AA==".to_string(),
        },
    };
    let value = serde_json::to_value(&message).expect("json");
    assert_eq!(value["type"], "session_deposit");
    assert_eq!(value["sessionId"], "s.a.1");
    assert_eq!(value["attachment"]["mimeType"], "image/jpeg");
    assert_eq!(value["attachment"]["name"], "page-1.jpg");
    let decoded: ClientMessage = serde_json::from_value(value).expect("round trip");
    assert_eq!(decoded, message);
}

#[test]
fn session_deposited_carries_the_stored_digest_and_its_session() {
    // One reference value, and it is exactly the one a later `session_send`
    // carries: session, digest and size together, so a digest is never
    // handled without the session it resolves in.
    let reference = AttachmentReference {
        session_id: "s.a.1".to_string(),
        digest: "a".repeat(64),
        stored_bytes: 4096,
    };
    let value = serde_json::to_value(DaemonMessage::SessionDeposited {
        id: 4,
        reference: reference.clone(),
    })
    .expect("json");
    assert_eq!(value["type"], "session_deposited");
    assert_eq!(value["reference"]["sessionId"], "s.a.1");
    assert_eq!(value["reference"]["digest"], "a".repeat(64));
    assert_eq!(value["reference"]["storedBytes"], 4096);
    let decoded: DaemonMessage = serde_json::from_value(value).expect("round trip");
    assert_eq!(
        decoded,
        DaemonMessage::SessionDeposited { id: 4, reference }
    );
}

#[test]
fn session_attachment_read_names_itself_and_is_a_read() {
    // A read returns bytes but writes nothing, so it sits on the read
    // side of `is_state_changing`: an audit row for it would be a disk
    // sink, and the gate refuses peers before any row is written.
    let read = ClientMessage::SessionAttachmentRead {
        id: 5,
        reference: AttachmentReference {
            session_id: "s.a.1".to_string(),
            digest: "b".repeat(64),
            stored_bytes: 512,
        },
    };
    assert_eq!(read.name(), "SessionAttachmentRead");
    assert!(!read.is_state_changing());
    assert_eq!(read.request_id(), Some(5));
    assert_eq!(read.idempotency_key(), None);
}

#[test]
fn session_attachment_read_round_trips_with_a_camel_case_envelope() {
    let message = ClientMessage::SessionAttachmentRead {
        id: 5,
        reference: AttachmentReference {
            session_id: "s.a.1".to_string(),
            digest: "b".repeat(64),
            stored_bytes: 512,
        },
    };
    let value = serde_json::to_value(&message).expect("json");
    assert_eq!(value["type"], "session_attachment_read");
    assert_eq!(value["reference"]["sessionId"], "s.a.1");
    assert_eq!(value["reference"]["digest"], "b".repeat(64));
    assert_eq!(value["reference"]["storedBytes"], 512);
    let decoded: ClientMessage = serde_json::from_value(value).expect("round trip");
    assert_eq!(decoded, message);
}

#[test]
fn session_attachment_carries_the_store_s_bytes_and_mime() {
    let attachment = StoredAttachment {
        mime_type: "text/markdown".to_string(),
        data: "aGk=".to_string(),
    };
    let value = serde_json::to_value(DaemonMessage::SessionAttachment {
        id: 6,
        attachment: attachment.clone(),
    })
    .expect("json");
    assert_eq!(value["type"], "session_attachment");
    assert_eq!(value["attachment"]["mimeType"], "text/markdown");
    assert_eq!(value["attachment"]["data"], "aGk=");
    let decoded: DaemonMessage = serde_json::from_value(value).expect("round trip");
    assert_eq!(
        decoded,
        DaemonMessage::SessionAttachment { id: 6, attachment }
    );
}

#[test]
fn session_send_with_references_round_trips_beside_inline_attachments() {
    // The inline list is untouched; the references are a second list in
    // the same frame and both survive the round trip.
    let reference = |seed: char| AttachmentReference {
        session_id: "s.a.1".to_string(),
        digest: seed.to_string().repeat(64),
        stored_bytes: 2048,
    };
    let message = ClientMessage::SessionSend {
        id: 7,
        session_id: "s.a.1".to_string(),
        subscription_id: 11,
        text: "summarise the deck".to_string(),
        attachments: vec![PromptAttachment {
            name: "cover.png".to_string(),
            mime_type: "image/png".to_string(),
            data: "AA==".to_string(),
        }],
        active_turn_behavior: None,
        attachment_references: vec![reference('a'), reference('b')],
        idempotency_key: None,
    };
    let value = serde_json::to_value(&message).expect("json");
    assert_eq!(value["attachments"][0]["name"], "cover.png");
    assert_eq!(value["attachmentReferences"][0]["digest"], "a".repeat(64));
    assert_eq!(value["attachmentReferences"][0]["sessionId"], "s.a.1");
    assert_eq!(value["attachmentReferences"][0]["storedBytes"], 2048);
    let decoded: ClientMessage = serde_json::from_value(value).expect("round trip");
    assert_eq!(decoded, message);
}

#[test]
fn an_old_send_frame_without_references_still_deserializes() {
    // `#[serde(default)]` is what keeps recorded journal frames (and v4
    // clients) readable without moving the journal version.
    let frame = r#"{"type":"session_send","id":7,"sessionId":"s.a.1","subscriptionId":11,"text":"hello","attachments":[{"name":"a.png","mimeType":"image/png","data":"AA=="}]}"#;
    let message: ClientMessage = serde_json::from_str(frame).expect("old frame");
    match message {
        ClientMessage::SessionSend {
            attachment_references,
            attachments,
            ..
        } => {
            assert!(attachment_references.is_empty());
            assert_eq!(attachments.len(), 1);
        }
        other => panic!("expected SessionSend, got {other:?}"),
    }
}

#[test]
fn session_send_with_no_references_omits_the_field() {
    let value = serde_json::to_value(ClientMessage::SessionSend {
        id: 7,
        session_id: "s.a.1".to_string(),
        subscription_id: 11,
        text: "hello".to_string(),
        attachments: Vec::new(),
        active_turn_behavior: None,
        attachment_references: Vec::new(),
        idempotency_key: None,
    })
    .expect("json");
    assert!(
        value.get("attachmentReferences").is_none(),
        "an empty reference list must not add a field to every send frame"
    );
}

#[test]
fn session_state_broadcast_is_a_compact_event_snapshot() {
    let message = DaemonMessage::Event(SessionEventEnvelope {
        session_id: String::new(),
        generation: 0,
        transcript_seq: None,
        event: SessionEvent::SessionsSnapshot {
            sessions: vec![SessionStateSnapshot {
                id: "s.client.1".to_string(),
                workspace_id: Some("workspace-1".to_string()),
                kind: SessionKind::Terminal,
                title: "Terminal".to_string(),
                state: SessionState::Silent { generation: 3 },
                elapsed_ms: Some(300_001),
                attention: None,
                origin: crate::SessionOrigin::peer("device-phone", PeerRole::Client),
                display_name: None,
                created_by: None,
                profile_id: None,
                context_id: None,
                unattended: UnattendedState::No,
                labels: Default::default(),
                delegation: None,

                activity: None,
            }],
        },
    });
    let value = serde_json::to_value(message).expect("json");
    assert_eq!(value["event"]["type"], "sessions_snapshot");
    assert_eq!(value["event"]["sessions"][0]["id"], "s.client.1");
    assert_eq!(value["event"]["sessions"][0]["title"], "Terminal");
    assert_eq!(value["event"]["sessions"][0]["state"]["type"], "silent");
    assert_eq!(value["event"]["sessions"][0]["elapsedMs"], 300_001);
    assert_eq!(value["event"]["sessions"][0]["workspaceId"], "workspace-1");
    assert_eq!(value["event"]["sessions"][0]["kind"], "terminal");
}

#[test]
fn attach_cursor_carries_generation_and_seq() {
    let value = serde_json::to_value(ClientMessage::SessionAttach {
        id: 3,
        session_id: "s.a.1".to_string(),
        subscription_id: 12,
        from_cursor: Some(Cursor {
            generation: 2,
            seq: 40,
        }),
    })
    .expect("json");
    assert_eq!(value["fromCursor"]["generation"], 2);
    assert_eq!(value["fromCursor"]["seq"], 40);
}

#[test]
fn subscription_identity_is_explicit_in_attach_reply_claim_and_events() {
    let attach = ClientMessage::SessionAttach {
        id: 3,
        session_id: "s.a.1".to_string(),
        subscription_id: 12,
        from_cursor: None,
    };
    let attach_json = serde_json::to_value(&attach).expect("attach json");
    assert_eq!(attach_json["type"], "session_attach");
    assert_eq!(attach_json["subscriptionId"], 12);
    assert_eq!(attach.request_id(), Some(3));

    let claim = ClientMessage::SessionClaim {
        id: 4,
        session_id: "s.a.1".to_string(),
        subscription_id: 12,
    };
    let claim_json = serde_json::to_value(&claim).expect("claim json");
    assert_eq!(claim_json["type"], "session_claim");
    assert_eq!(claim_json["subscriptionId"], 12);
    assert_eq!(claim.request_id(), Some(4));

    let attached = DaemonMessage::SessionAttached {
        id: 4,
        subscription_id: 12,
    };
    let attached_json = serde_json::to_value(&attached).expect("attach reply json");
    assert_eq!(attached_json["type"], "session_attached");
    assert_eq!(attached_json["subscriptionId"], 12);

    let event = DaemonMessage::SubscriptionEvent {
        subscription_id: 12,
        envelope: SessionEventEnvelope {
            session_id: "s.a.1".to_string(),
            generation: 1,
            transcript_seq: None,
            event: SessionEvent::AgentMessage {
                message_id: None,
                text: "hello".to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
        },
    };
    let event_json = serde_json::to_value(&event).expect("subscription event json");
    assert_eq!(event_json["type"], "subscription_event");
    assert_eq!(event_json["subscriptionId"], 12);
    assert_eq!(event_json["envelope"]["sessionId"], "s.a.1");
    assert_eq!(
        serde_json::from_value::<DaemonMessage>(event_json).expect("event round trip"),
        event
    );
}

#[test]
fn presence_carries_focus_and_visibility_per_connection() {
    let message = ClientMessage::SessionsPresence {
        id: 4,
        focused_session_id: Some("s.a.1".to_string()),
        app_visible: true,
    };
    let value = serde_json::to_value(&message).expect("presence json");
    assert_eq!(value["type"], "sessions_presence");
    assert_eq!(value["focusedSessionId"], "s.a.1");
    assert_eq!(value["appVisible"], true);
    let decoded: ClientMessage = serde_json::from_value(value).expect("presence round trip");
    assert_eq!(decoded, message);
}

#[test]
fn project_workspace_wire_fields_are_camel_case() {
    let request = ClientMessage::WorkspaceCreate {
        id: 7,
        project_id: "p.one".to_string(),
        isolation: WorkspaceIsolation::Local,
        branch: Some("main".to_string()),
    };
    let value = serde_json::to_value(&request).expect("workspace request json");
    assert_eq!(value["type"], "workspace_create");
    assert_eq!(value["projectId"], "p.one");
    assert_eq!(value["isolation"], "local");
    assert_eq!(value["branch"], "main");
    assert!(value.get("project_id").is_none());

    let workspace = Workspace {
        id: "w.one".to_string(),
        project_id: "p.one".to_string(),
        title: "Project".to_string(),
        isolation: WorkspaceIsolation::Local,
        path: r"C:\code\Project".to_string(),
    };
    let reply = serde_json::to_value(DaemonMessage::Workspace { id: 7, workspace })
        .expect("workspace reply json");
    assert_eq!(reply["workspace"]["projectId"], "p.one");
    assert_eq!(reply["workspace"]["path"], r"C:\code\Project");
    assert!(reply["workspace"].get("project_id").is_none());

    let delete = ClientMessage::WorkspaceDelete {
        id: 8,
        workspace_id: "w.one".to_string(),
        force: true,
    };
    let value = serde_json::to_value(&delete).expect("workspace delete json");
    assert_eq!(value["type"], "workspace_delete");
    assert_eq!(value["workspaceId"], "w.one");
    assert_eq!(value["force"], true);
}

#[test]
fn create_send_permission_carry_idempotency_key() {
    let create = ClientMessage::SessionCreate {
        id: 1,
        workspace_id: None,
        kind: SessionKind::Terminal,
        provider: None,
        mode: None,
        display_name: None,
        idempotency_key: Some("k1".to_string()),
    };
    let send = ClientMessage::SessionSend {
        id: 2,
        session_id: "s.a.1".to_string(),
        subscription_id: 12,
        text: "x".to_string(),
        attachments: Vec::new(),
        active_turn_behavior: None,
        attachment_references: Vec::new(),
        idempotency_key: Some("k2".to_string()),
    };
    let perm = ClientMessage::SessionPermissionRespond {
        id: 3,
        session_id: "s.a.1".to_string(),
        subscription_id: 12,
        request_id: "r1".to_string(),
        outcome: PermissionOutcome::AllowOnce,
        option_id: Some("allow-once".to_string()),
        answer: None,
        idempotency_key: Some("k3".to_string()),
    };
    assert_eq!(create.idempotency_key(), Some("k1"));
    assert_eq!(send.idempotency_key(), Some("k2"));
    assert_eq!(perm.idempotency_key(), Some("k3"));
    assert_eq!(
        serde_json::to_value(&perm).expect("json")["outcome"],
        "allow_once"
    );
    assert_eq!(
        serde_json::to_value(&perm).expect("json")["optionId"],
        "allow-once"
    );
    let decoded: ClientMessage = serde_json::from_value(serde_json::to_value(&perm).expect("json"))
        .expect("permission response round trip");
    assert_eq!(decoded, perm);
}

#[test]
fn permission_resolved_reports_the_selected_option() {
    let event = DaemonMessage::Event(SessionEventEnvelope {
        session_id: "s.a.1".to_string(),
        generation: 1,
        transcript_seq: None,
        event: SessionEvent::PermissionResolved {
            tool_call_id: "tool-1".to_string(),
            selected_option_id: Some("allow-once".to_string()),
            selected_option_kind: Some("allow_once".to_string()),
            selected_option_name: Some("Allow once".to_string()),
            answered_by: None,
        },
    });
    let value = serde_json::to_value(&event).expect("permission resolved json");
    assert_eq!(value["event"]["selectedOptionId"], "allow-once");
    assert_eq!(value["event"]["selectedOptionKind"], "allow_once");
    assert_eq!(value["event"]["selectedOptionName"], "Allow once");
    assert_eq!(
        serde_json::from_value::<DaemonMessage>(value).expect("permission resolved round trip"),
        event
    );
}

#[test]
fn legacy_permission_frames_without_option_fields_still_parse() {
    // Bytes, not Rust-to-Rust: an older client answers without
    // `optionId`, and an older daemon resolves without the option triple.
    let respond: ClientMessage = serde_json::from_str(
            r#"{"type":"session_permission_respond","id":3,"sessionId":"s.a.1","subscriptionId":12,"requestId":"r1","outcome":"allow_once"}"#,
        )
        .expect("legacy permission response parses");
    assert_eq!(
        respond,
        ClientMessage::SessionPermissionRespond {
            id: 3,
            session_id: "s.a.1".to_string(),
            subscription_id: 12,
            request_id: "r1".to_string(),
            outcome: PermissionOutcome::AllowOnce,
            option_id: None,
            answer: None,
            idempotency_key: None,
        }
    );

    let resolved: DaemonMessage = serde_json::from_str(
            r#"{"type":"event","sessionId":"s.a.1","generation":1,"event":{"type":"permission_resolved","toolCallId":"tool-1"}}"#,
        )
        .expect("legacy permission resolved parses");
    assert_eq!(
        resolved,
        DaemonMessage::Event(SessionEventEnvelope {
            session_id: "s.a.1".to_string(),
            generation: 1,
            transcript_seq: None,
            event: SessionEvent::PermissionResolved {
                tool_call_id: "tool-1".to_string(),
                selected_option_id: None,
                selected_option_kind: None,
                selected_option_name: None,
                answered_by: None,
            },
        })
    );
    let value = serde_json::to_value(&resolved).expect("resolved json");
    assert!(value["event"].get("selectedOptionId").is_none());
    assert!(value["event"].get("selectedOptionKind").is_none());
    assert!(value["event"].get("selectedOptionName").is_none());
}

#[test]
fn retention_mutations_carry_idempotency_keys() {
    let retention = ClientMessage::JournalRetentionSet {
        id: 4,
        max_age_ms: None,
        max_bytes: Some(10),
        max_sessions: None,
        session_max_bytes: None,
        idempotency_key: Some("retention-key".to_string()),
    };
    let delete = ClientMessage::SessionDelete {
        id: 5,
        session_id: "s.a.1".to_string(),
        idempotency_key: Some("delete-key".to_string()),
    };
    let close = ClientMessage::SessionClose {
        id: 6,
        session_id: "s.a.1".to_string(),
        idempotency_key: Some("close-key".to_string()),
    };
    assert_eq!(retention.idempotency_key(), Some("retention-key"));
    assert_eq!(delete.idempotency_key(), Some("delete-key"));
    assert_eq!(close.idempotency_key(), Some("close-key"));
    assert_eq!(
        serde_json::to_value(retention).expect("json")["idempotencyKey"],
        "retention-key"
    );
    assert_eq!(
        serde_json::to_value(delete).expect("json")["idempotencyKey"],
        "delete-key"
    );
    assert_eq!(
        serde_json::to_value(close).expect("json")["idempotencyKey"],
        "close-key"
    );
}

#[test]
fn ping_roundtrip() {
    let msg = ClientMessage::Ping { id: 7 };
    let encoded = serde_json::to_string(&msg).expect("json");
    assert!(!encoded.contains('\n'));
    let decoded: ClientMessage = serde_json::from_str(&encoded).expect("parse");
    assert_eq!(msg, decoded);
}

#[test]
fn session_set_model_round_trips_with_optional_fields() {
    let msg = ClientMessage::SessionSetModel {
        id: 8,
        session_id: "s.a.1".to_string(),
        model_id: Some("grok-4.5".to_string()),
        effort: Some("low".to_string()),
    };
    let value = serde_json::to_value(&msg).expect("json");
    assert_eq!(value["type"], "session_set_model");
    assert_eq!(value["sessionId"], "s.a.1");
    assert_eq!(value["modelId"], "grok-4.5");
    assert_eq!(value["effort"], "low");
    let encoded = serde_json::to_string(&msg).expect("json");
    let decoded: ClientMessage = serde_json::from_str(&encoded).expect("parse");
    assert_eq!(decoded, msg);

    let effort_only = ClientMessage::SessionSetModel {
        id: 9,
        session_id: "s.a.1".to_string(),
        model_id: None,
        effort: Some("high".to_string()),
    };
    let effort_only_value = serde_json::to_value(effort_only).expect("json");
    assert!(effort_only_value.get("modelId").is_none());
}

#[test]
fn session_set_mode_round_trips_with_camel_case_fields() {
    let message = ClientMessage::SessionSetMode {
        id: 10,
        session_id: "s.a.1".to_string(),
        mode_id: "acceptEdits".to_string(),
    };
    let value = serde_json::to_value(&message).expect("json");
    assert_eq!(value["type"], "session_set_mode");
    assert_eq!(value["sessionId"], "s.a.1");
    assert_eq!(value["modeId"], "acceptEdits");
    assert_eq!(message.request_id(), Some(10));
    assert_eq!(message.idempotency_key(), None);
    assert_eq!(
        serde_json::from_value::<ClientMessage>(value).expect("round trip"),
        message
    );
}

#[test]
fn session_create_round_trips_an_optional_mode() {
    let message = ClientMessage::SessionCreate {
        id: 11,
        workspace_id: None,
        kind: SessionKind::Claude,
        provider: None,
        mode: Some("plan".to_string()),
        display_name: None,
        idempotency_key: None,
    };
    let value = serde_json::to_value(&message).expect("json");
    assert_eq!(value["mode"], "plan");
    assert_eq!(
        serde_json::from_value::<ClientMessage>(value).expect("round trip"),
        message
    );
}

#[test]
fn session_report_agent_round_trips_with_herdr_payload() {
    let msg = ClientMessage::SessionReportAgent {
        id: 11,
        session_id: "s.client.1".to_string(),
        source: "devboule:stub".to_string(),
        agent: "stub".to_string(),
        state: AgentActivityState::Working,
        message: None,
        seq: Some(3),
        agent_session_id: Some("agent-1".to_string()),
        agent_session_path: None,
        session_start_source: Some("startup".to_string()),
    };
    let value = serde_json::to_value(&msg).expect("json");
    assert_eq!(value["type"], "session_report_agent");
    assert_eq!(value["sessionId"], "s.client.1");
    assert_eq!(value["source"], "devboule:stub");
    assert_eq!(value["agent"], "stub");
    assert_eq!(value["state"], "working");
    assert_eq!(value["seq"], 3);
    assert_eq!(value["agentSessionId"], "agent-1");
    assert_eq!(value["sessionStartSource"], "startup");
    assert!(value.get("message").is_none());
    assert!(value.get("agentSessionPath").is_none());
    assert_eq!(msg.request_id(), Some(11));
    assert_eq!(msg.idempotency_key(), None);
    let encoded = serde_json::to_string(&msg).expect("json");
    assert!(!encoded.contains('\n'));
    let decoded: ClientMessage = serde_json::from_str(&encoded).expect("parse");
    assert_eq!(decoded, msg);
}

#[test]
fn invoke_is_the_plugin_tenant_on_the_same_frames() {
    let msg = ClientMessage::Invoke {
        id: 11,
        method: crate::caps::WORKSPACE_ROOT.to_string(),
        payload: None,
    };
    let value = serde_json::to_value(&msg).expect("json");
    assert_eq!(value["type"], "invoke");
    assert_eq!(value["id"], 11);
    assert_eq!(value["method"], "workspace.root");
    assert!(value.get("payload").is_none());
    assert_eq!(msg.request_id(), Some(11));

    let reply = DaemonMessage::InvokeResult {
        id: 11,
        value: serde_json::json!({
            "root": r"C:\repo",
            "status": "ok"
        }),
    };
    let encoded = serde_json::to_string(&reply).expect("json");
    assert!(!encoded.contains('\n'));
    let decoded: DaemonMessage = serde_json::from_str(&encoded).expect("parse");
    assert_eq!(decoded, reply);
    let wire = serde_json::to_value(&reply).expect("json");
    assert_eq!(wire["type"], "invoke_result");
    assert_eq!(wire["value"]["root"], r"C:\repo");
    assert_eq!(wire["value"]["status"], "ok");
}

#[test]
fn journal_stats_round_trips_with_camel_case_wire_names() {
    let stats = JournalStats {
        accepted_frames: 12,
        accepted_bytes: 4096,
        committed_frames: 10,
        committed_bytes: 3840,
        failed_frames: 2,
    };
    let encoded = serde_json::to_value(stats).expect("json");
    assert_eq!(encoded["acceptedFrames"], 12);
    assert_eq!(encoded["acceptedBytes"], 4096);
    assert_eq!(encoded["committedFrames"], 10);
    assert_eq!(encoded["committedBytes"], 3840);
    assert_eq!(encoded["failedFrames"], 2);
    let decoded: JournalStats = serde_json::from_value(encoded).expect("parse");
    assert_eq!(decoded, stats);
}

#[test]
fn status_body_treats_journal_stats_as_optional_for_older_daemons() {
    // A daemon predating the field must still parse; the client must
    // read its absence as "no journal writer", not as a wire error.
    let older_daemon_frame = serde_json::json!({
        "type": "status",
        "id": 5,
        "instanceId": "i",
        "protocolVersion": 2,
        "daemonVersion": "0.0.0",
        "pid": 42,
        "uptimeMs": 7,
        "clients": 1,
        "sessions": 2,
        "capabilities": [],
        "peakRingBytes": 0,
        "ringEvictedBytes": 0,
        "ringDroppedFrames": 0
    });
    let decoded = serde_json::from_value::<DaemonStatusBody>(older_daemon_frame)
        .expect("a status frame without journalStats");
    assert!(decoded.journal_stats.is_none());
}

#[test]
fn boxing_journal_stats_and_remote_does_not_change_the_wire() {
    // The two fields are `Box`ed purely to keep this frame small: the Tauri
    // client stores the whole body inside an enum and
    // `clippy::large_enum_variant` measures that enum. `serde` treats
    // `Box<T>` as transparent, so the JSON must be byte-identical to the
    // inline shape, including the camelCase names and the nested `remote`
    // object.
    let body = DaemonStatusBody {
        instance_id: "i".to_string(),
        protocol_version: 4,
        daemon_version: "0.0.0".to_string(),
        pid: 1,
        uptime_ms: 2,
        clients: 0,
        local_clients: 0,
        sessions: 0,
        agents: Some(0),
        terminals: Some(0),
        capabilities: Vec::new(),
        peak_ring_bytes: 0,
        ring_evicted_bytes: 0,
        ring_dropped_frames: 0,
        journal_error: None,
        journal_stats: Some(Box::new(JournalStats {
            accepted_frames: 1,
            accepted_bytes: 2,
            committed_frames: 3,
            committed_bytes: 4,
            failed_frames: 5,
        })),
        secret_store: Some("file".to_string()),
        remote: Some(Box::new(RemoteState::disabled("no tailscale"))),
    };
    let json = serde_json::to_value(&body).expect("json");
    assert_eq!(json["journalStats"]["acceptedFrames"], 1);
    assert_eq!(json["journalStats"]["failedFrames"], 5);
    assert_eq!(json["secretStore"], "file");
    assert_eq!(json["remote"]["state"], "disabled");
    assert_eq!(json["remote"]["reason"], "no tailscale");

    let decoded: DaemonStatusBody = serde_json::from_value(json.clone()).expect("back");
    assert_eq!(
        decoded.journal_stats.as_deref(),
        body.journal_stats.as_deref()
    );
    assert_eq!(decoded.remote.as_deref(), body.remote.as_deref());
    assert_eq!(
        serde_json::to_value(&decoded).expect("json"),
        json,
        "the round trip must not move a byte"
    );
}

#[test]
fn pty_output_newlines_are_escaped_in_compact_json() {
    let event = SessionEvent::Output {
        seq: 1,
        data: "line1\nline2".to_string(),
    };
    let encoded = serde_json::to_string(&event).expect("json");
    assert!(
        !encoded.contains('\n'),
        "compact JSON must not contain a raw newline or NDJSON framing splits the event"
    );
    assert!(encoded.contains("\\n"));
}

#[test]
fn journal_commands_round_trip_the_amended_usage_shape() {
    let set = ClientMessage::JournalRetentionSet {
        id: 17,
        session_max_bytes: Some(0),
        max_bytes: Some(8_000),
        max_sessions: None,
        max_age_ms: Some(0),
        idempotency_key: None,
    };
    let wire = serde_json::to_value(&set).expect("json");
    assert_eq!(wire["type"], "journal_retention_set");
    assert_eq!(wire["sessionMaxBytes"], 0);
    assert_eq!(wire["maxBytes"], 8_000);
    assert!(wire.get("maxSessions").is_none());

    let usage = DaemonMessage::JournalUsage {
        id: 17,
        usage: JournalUsage {
            total_bytes: 10,
            session_count: 2,
            deleted_by_user: 1,
            deleted_by_retention: 4,
            unreclaimable: Unreclaimable {
                bytes_over: 3,
                sessions_over: 4,
                aged_out: 5,
            },
            limits: JournalLimits {
                snapshot_every_bytes: 1,
                session_max_bytes: 2,
                max_bytes: 3,
                max_sessions: 4,
                max_age_ms: 5,
            },
            per_session: vec![JournalSessionUsage {
                id: "s.1".to_string(),
                title: "Terminal".to_string(),
                display_name: Some("worker one".to_string()),
                kind: SessionKind::Terminal,
                bytes: 6,
                updated_at_ms: 7,
            }],
        },
    };
    let encoded = serde_json::to_string(&usage).expect("json");
    assert!(encoded.contains("\"deletedByUser\":1"));
    assert!(encoded.contains("\"deletedByRetention\":4"));
    assert!(encoded.contains("\"unreclaimable\":{"));
    assert!(encoded.contains("\"bytesOver\":3"));
    assert!(encoded.contains("\"sessionsOver\":4"));
    assert!(encoded.contains("\"agedOut\":5"));
    assert!(encoded.contains("\"displayName\":\"worker one\""));
    assert_eq!(
        serde_json::from_str::<DaemonMessage>(&encoded).expect("round trip"),
        usage
    );
}

/// The name History shows travels in the same dialect as the rest of the
/// frame: camel case, and absent rather than null when the row has no name
/// of its own — the app's rule is "absent means the fallback name", and a
/// null would be a third state it does not render. A frame from a client
/// that predates the field still decodes, as an unnamed row.
#[test]
fn a_journal_usage_row_carries_the_display_name_in_camel_case() {
    let named = JournalSessionUsage {
        id: "s.1".to_string(),
        title: "worker".to_string(),
        display_name: Some("worker one".to_string()),
        kind: SessionKind::Terminal,
        bytes: 6,
        updated_at_ms: 7,
    };
    let value = serde_json::to_value(&named).expect("json");
    assert_eq!(
        value.get("displayName").and_then(|name| name.as_str()),
        Some("worker one")
    );

    let unnamed = JournalSessionUsage {
        display_name: None,
        ..named.clone()
    };
    let value = serde_json::to_value(&unnamed).expect("json");
    assert!(value.get("displayName").is_none(), "{value}");

    let older: JournalSessionUsage = serde_json::from_value(serde_json::json!({
        "id": "s.1",
        "title": "worker",
        "kind": "terminal",
        "bytes": 6,
        "updatedAtMs": 7
    }))
    .expect("a frame without the field is still a row");
    assert_eq!(older.display_name, None);
}

#[test]
fn tool_policy_wire_contract_round_trips_with_its_exact_field_names() {
    // The Settings panel is built against this JSON, and a rename here is
    // a silently inert toggle there, so the names are asserted on the
    // serialised form rather than on the Rust fields.
    let get = serde_json::to_value(ClientMessage::ToolPolicyGet { id: 31 }).expect("json");
    assert_eq!(
        get,
        serde_json::json!({"type": "tool_policy_get", "id": 31})
    );

    let set = ClientMessage::ToolPolicySet {
        id: 32,
        provider_id: "claude".to_string(),
        enabled: Some(false),
        disabled_tools: vec!["devboule_list_agents".to_string()],
    };
    let set_json = serde_json::to_value(&set).expect("json");
    assert_eq!(set_json["type"], "tool_policy_set");
    assert_eq!(set_json["providerId"], "claude");
    assert_eq!(set_json["enabled"], false);
    assert_eq!(set_json["disabledTools"][0], "devboule_list_agents");
    assert_eq!(
        serde_json::from_value::<ClientMessage>(set_json).expect("back"),
        set
    );

    // An absent `enabled` is the app's `null` and means enabled; so does
    // an explicit `null`, because the field has a serde default.
    for omitted in [
        serde_json::json!({"type": "tool_policy_set", "id": 33, "providerId": "pi"}),
        serde_json::json!({
            "type": "tool_policy_set",
            "id": 33,
            "providerId": "pi",
            "enabled": null
        }),
    ] {
        assert_eq!(
            serde_json::from_value::<ClientMessage>(omitted).expect("absent enabled"),
            ClientMessage::ToolPolicySet {
                id: 33,
                provider_id: "pi".to_string(),
                enabled: None,
                disabled_tools: Vec::new(),
            }
        );
    }

    let reply = DaemonMessage::ToolPolicy {
        id: 34,
        policies: vec![ToolPolicyEntry {
            provider_id: "claude".to_string(),
            enabled: None,
            disabled_tools: Vec::new(),
        }],
    };
    let reply_json = serde_json::to_value(&reply).expect("json");
    assert_eq!(reply_json["type"], "tool_policy");
    assert_eq!(reply_json["policies"][0]["providerId"], "claude");
    assert_eq!(
        reply_json["policies"][0]["enabled"],
        serde_json::Value::Null
    );
    assert!(
        reply_json["policies"][0].get("disabledTools").is_none(),
        "an empty disabled list is omitted, not sent as []"
    );
    assert_eq!(
        serde_json::from_value::<DaemonMessage>(reply_json).expect("back"),
        reply
    );

    assert_eq!(
        serde_json::to_value(DaemonMessage::ToolPolicySetOk { id: 35 }).expect("json"),
        serde_json::json!({"type": "tool_policy_set_ok", "id": 35})
    );
}

#[test]
fn agent_profiles_wire_contract_round_trips_with_its_exact_field_names() {
    // The Settings → Agents form is built against this JSON, and a rename
    // here is a silently dropped profile field there, so the names are
    // asserted on the serialised form rather than on the Rust fields.
    let get = serde_json::to_value(ClientMessage::AgentProfilesGet { id: 41 }).expect("json");
    assert_eq!(
        get,
        serde_json::json!({"type": "agent_profiles_get", "id": 41})
    );

    let mut features = serde_json::Map::new();
    features.insert("autoAccept".to_string(), serde_json::json!(true));
    let document = AgentProfilesDocument {
        profiles: vec![AgentProfile {
            id: "p-1".to_string(),
            name: "Reviewer".to_string(),
            icon: Some("eye".to_string()),
            note: "Use for a second opinion.".to_string(),
            spawn_prompt: String::new(),
            provider: "claude".to_string(),
            model: "opus".to_string(),
            mode_id: "default".to_string(),
            thinking_option_id: Some("high".to_string()),
            features,
            tool_overlay: vec!["devboule_create_agent".to_string()],
            enabled_for_agents: true,
            idle_close_minutes: None,
        }],
        standing_instructions: "Report in your final message.".to_string(),
    };
    let set = ClientMessage::AgentProfilesSet {
        id: 42,
        document: document.clone(),
    };
    let set_json = serde_json::to_value(&set).expect("json");
    assert_eq!(set_json["type"], "agent_profiles_set");
    assert_eq!(set_json["document"]["profiles"][0]["id"], "p-1");
    assert_eq!(set_json["document"]["profiles"][0]["modeId"], "default");
    assert_eq!(
        set_json["document"]["profiles"][0]["thinkingOptionId"],
        "high"
    );
    assert_eq!(
        set_json["document"]["profiles"][0]["enabledForAgents"],
        true
    );
    assert_eq!(
        set_json["document"]["profiles"][0]["toolOverlay"][0],
        "devboule_create_agent"
    );
    assert_eq!(
        set_json["document"]["profiles"][0]["features"]["autoAccept"],
        true
    );
    assert_eq!(
        set_json["document"]["standingInstructions"],
        "Report in your final message."
    );
    assert_eq!(
        serde_json::from_value::<ClientMessage>(set_json).expect("back"),
        set
    );

    // A profile whose optional halves are absent round-trips as absent: an
    // empty note and an empty feature map are omitted, never sent as `""`
    // and `{}`, because the app renders presence and a fabricated empty
    // value is a different profile.
    let bare = serde_json::json!({
        "type": "agent_profiles_set",
        "id": 43,
        "document": {
            "profiles": [{
                "id": "",
                "name": "Bare",
                "provider": "pi",
                "model": "gpt-5",
                "modeId": "ask",
                "enabledForAgents": false
            }],
            "standingInstructions": ""
        }
    });
    assert_eq!(
        serde_json::from_value::<ClientMessage>(bare).expect("bare profile"),
        ClientMessage::AgentProfilesSet {
            id: 43,
            document: AgentProfilesDocument {
                profiles: vec![AgentProfile {
                    id: String::new(),
                    name: "Bare".to_string(),
                    icon: None,
                    note: String::new(),
                    spawn_prompt: String::new(),
                    provider: "pi".to_string(),
                    model: "gpt-5".to_string(),
                    mode_id: "ask".to_string(),
                    thinking_option_id: None,
                    features: serde_json::Map::new(),
                    tool_overlay: Vec::new(),
                    enabled_for_agents: false,
                    idle_close_minutes: None,
                }],
                standing_instructions: String::new(),
            },
        }
    );

    // An unknown field is refused rather than dropped, at both levels, and
    // the refusal names it: a dropped field would be a profile the human
    // saved and the daemon quietly did not keep.
    for unknown in [
        serde_json::json!({
            "type": "agent_profiles_set",
            "id": 44,
            "document": {
                "profiles": [],
                "standingInstructions": "",
                "standing": "typo"
            }
        }),
        serde_json::json!({
            "type": "agent_profiles_set",
            "id": 45,
            "document": {
                "profiles": [{
                    "id": "p-2",
                    "name": "Bare",
                    "provider": "pi",
                    "model": "gpt-5",
                    "modeId": "ask",
                    "enabledForAgents": false,
                    "mode": "ask"
                }],
                "standingInstructions": ""
            }
        }),
    ] {
        let refused = serde_json::from_value::<ClientMessage>(unknown.clone())
            .expect_err("an unknown field must be refused");
        let named = if unknown["document"].get("standing").is_some() {
            "standing"
        } else {
            "mode"
        };
        assert!(
            refused.to_string().contains(named),
            "the refusal must name {named}: {refused}"
        );
    }

    let reply = DaemonMessage::AgentProfiles {
        id: 46,
        document: document.clone(),
    };
    let reply_json = serde_json::to_value(&reply).expect("json");
    assert_eq!(reply_json["type"], "agent_profiles");
    assert_eq!(reply_json["document"]["profiles"][0]["name"], "Reviewer");
    assert_eq!(
        serde_json::from_value::<DaemonMessage>(reply_json).expect("back"),
        reply
    );

    assert_eq!(
        serde_json::to_value(DaemonMessage::AgentProfilesSetOk { id: 47 }).expect("json"),
        serde_json::json!({"type": "agent_profiles_set_ok", "id": 47})
    );

    // The audit and the rate-limit sides of the two variants: a read may
    // not produce an audit row, a write must.
    assert!(!ClientMessage::AgentProfilesGet { id: 1 }.is_state_changing());
    assert!(ClientMessage::AgentProfilesSet {
        id: 1,
        document: AgentProfilesDocument::default(),
    }
    .is_state_changing());
    assert_eq!(
        ClientMessage::AgentProfilesGet { id: 1 }.name(),
        "AgentProfilesGet"
    );
    assert_eq!(
        ClientMessage::AgentProfilesSet {
            id: 1,
            document: AgentProfilesDocument::default(),
        }
        .name(),
        "AgentProfilesSet"
    );
}

/// The spawn prompt rides the profile as `spawnPrompt`, and its empty form
/// is **omitted** rather than sent as `""`: a document where no profile
/// carries one serialises exactly like the pre-field shape, which is what
/// keeps an older daemon — `deny_unknown_fields` and all — reading a newer
/// app's document until the first prompt is written. Its absence decodes as
/// none, so an old document loads unchanged.
#[test]
fn a_profile_spawn_prompt_round_trips_and_its_absence_is_none() {
    let mut carried = AgentProfile {
        id: "p-9".to_string(),
        name: "Sentinel".to_string(),
        icon: None,
        note: String::new(),
        spawn_prompt: "Check the diff before you report.".to_string(),
        provider: "claude".to_string(),
        model: "opus".to_string(),
        mode_id: "default".to_string(),
        thinking_option_id: None,
        features: serde_json::Map::new(),
        tool_overlay: Vec::new(),
        enabled_for_agents: true,
        idle_close_minutes: None,
    };
    let set = ClientMessage::AgentProfilesSet {
        id: 48,
        document: AgentProfilesDocument {
            profiles: vec![carried.clone()],
            standing_instructions: String::new(),
        },
    };
    let json = serde_json::to_value(&set).expect("json");
    assert_eq!(
        json["document"]["profiles"][0]["spawnPrompt"],
        "Check the diff before you report."
    );
    assert_eq!(
        serde_json::from_value::<ClientMessage>(json).expect("back"),
        set,
        "the carried prompt decodes to the same profile"
    );

    // Empty is omitted on the wire, never sent as an empty string.
    carried.spawn_prompt = String::new();
    let quiet = ClientMessage::AgentProfilesSet {
        id: 49,
        document: AgentProfilesDocument {
            profiles: vec![carried],
            standing_instructions: String::new(),
        },
    };
    let json = serde_json::to_value(&quiet).expect("json");
    assert!(
        json["document"]["profiles"][0].get("spawnPrompt").is_none(),
        "an empty prompt is omitted: {json}"
    );

    // ...and absence decodes as none: the pre-field document shape parses.
    let old = serde_json::json!({
        "type": "agent_profiles_set",
        "id": 50,
        "document": {
            "profiles": [{
                "id": "p-old",
                "name": "Old",
                "provider": "pi",
                "model": "gpt-5",
                "modeId": "ask",
                "enabledForAgents": false
            }],
            "standingInstructions": ""
        }
    });
    let decoded = serde_json::from_value::<ClientMessage>(old).expect("the old shape");
    let ClientMessage::AgentProfilesSet { document, .. } = &decoded else {
        panic!("the wrong variant decoded");
    };
    assert_eq!(document.profiles[0].spawn_prompt, "");
}

/// The idle-close timer's wire shape, for the reason the spawn prompt's is
/// pinned above: `Some(0)` is the timer **off**, so it has to survive the
/// `skip_serializing_if` that drops the default — an app's "never close this"
/// silently becoming "close after 30 minutes" is the bug this catches — and
/// the default itself is the key's absence, which decodes as none.
#[test]
fn an_idle_close_round_trip_keeps_zero_and_omits_the_default() {
    let base = AgentProfile {
        id: "p-i".to_string(),
        name: "Idle".to_string(),
        icon: None,
        note: String::new(),
        spawn_prompt: String::new(),
        provider: "claude".to_string(),
        model: "opus".to_string(),
        mode_id: "default".to_string(),
        thinking_option_id: None,
        features: serde_json::Map::new(),
        tool_overlay: Vec::new(),
        enabled_for_agents: true,
        idle_close_minutes: None,
    };
    let wire = |id: u64, minutes: Option<u32>| -> serde_json::Value {
        serde_json::to_value(ClientMessage::AgentProfilesSet {
            id,
            document: AgentProfilesDocument {
                profiles: vec![AgentProfile {
                    idle_close_minutes: minutes,
                    ..base.clone()
                }],
                standing_instructions: String::new(),
            },
        })
        .expect("json")
    };
    let decode = |json: serde_json::Value| -> AgentProfile {
        let ClientMessage::AgentProfilesSet { document, .. } =
            serde_json::from_value(json).expect("back")
        else {
            panic!("the wrong variant decoded");
        };
        document.profiles[0].clone()
    };

    // The default is the key's absence — an older document parses unchanged.
    let silent = wire(51, None);
    assert!(
        silent["document"]["profiles"][0]
            .get("idleCloseMinutes")
            .is_none(),
        "a profile that says nothing carries no key: {silent}"
    );
    assert_eq!(decode(silent).idle_close_minutes, None);

    // Zero is a value, not an absence: the off switch must reach the daemon.
    let off = wire(52, Some(0));
    assert_eq!(
        off["document"]["profiles"][0]["idleCloseMinutes"], 0,
        "{off}"
    );
    assert_eq!(decode(off).idle_close_minutes, Some(0));

    // And a number is its own number, all the way round.
    let custom = wire(53, Some(45));
    assert_eq!(custom["document"]["profiles"][0]["idleCloseMinutes"], 45);
    assert_eq!(decode(custom).idle_close_minutes, Some(45));
}

/// The wire's split, which the app's reader is written against: five fields
/// are skipped when they are empty — an absent `icon`/`spawnPrompt`/
/// `thinkingOptionId`/`features`/`toolOverlay` **is** the empty value — and
/// the identity, `note` and `enabledForAgents` are always there. Older builds
/// wrote documents carrying only the mandatory keys; such a document loads
/// unchanged and serialises back to the same shape. Reading an omitted field
/// without a default blanked the app on a live legacy profile, so this test
/// is the daemon's half of the agreement.
#[test]
fn a_profile_omits_its_empty_fields_and_never_the_ones_the_app_reads_as_required() {
    let old = serde_json::json!({
        "profiles": [{
            "id": "p-old",
            "name": "Night probe",
            "provider": "grok",
            "model": "grok-4",
            "modeId": "ask",
            "enabledForAgents": false
        }],
        "standingInstructions": ""
    });
    let document: AgentProfilesDocument = serde_json::from_value(old).expect("the old shape");
    assert!(document.profiles[0].features.is_empty());
    assert_eq!(
        document.profiles[0].note, "",
        "a missing note decodes as the empty note"
    );

    let wire = serde_json::to_value(&document).expect("json");
    let profile = &wire["profiles"][0];
    for omitted in [
        "icon",
        "spawnPrompt",
        "thinkingOptionId",
        "features",
        "toolOverlay",
    ] {
        assert!(
            profile.get(omitted).is_none(),
            "{omitted} is skipped when empty, so a reader must default it: {wire}"
        );
    }
    for always in [
        "id",
        "name",
        "note",
        "provider",
        "model",
        "modeId",
        "enabledForAgents",
    ] {
        assert!(
            profile.get(always).is_some(),
            "{always} is never omitted: {wire}"
        );
    }

    let back: AgentProfilesDocument = serde_json::from_value(wire).expect("decodes back");
    assert_eq!(
        back, document,
        "the wire shape round-trips to the same document"
    );
}

#[test]
fn delegation_wire_contract_round_trips_with_its_exact_field_names() {
    // The app's Settings switch and the roster's take-back are written
    // against this JSON, and the reply's `source` is the three-valued
    // answer the panel renders — so the names are asserted on the
    // serialised form, the same discipline the profiles contract above
    // applies.
    let get = serde_json::to_value(ClientMessage::DelegationGet { id: 51 }).expect("json");
    assert_eq!(get, serde_json::json!({"type": "delegation_get", "id": 51}));

    let set = ClientMessage::DelegationSet {
        id: 52,
        enabled: false,
    };
    let set_json = serde_json::to_value(&set).expect("json");
    assert_eq!(
        set_json,
        serde_json::json!({"type": "delegation_set", "id": 52, "enabled": false})
    );
    assert_eq!(
        serde_json::from_value::<ClientMessage>(set_json).expect("back"),
        set
    );

    for (source, wire) in [
        (DelegationSource::File, "file"),
        (DelegationSource::Default, "default"),
        (DelegationSource::Quarantined, "quarantined"),
    ] {
        let state = DaemonMessage::DelegationState {
            id: 53,
            enabled: false,
            source,
        };
        let state_json = serde_json::to_value(&state).expect("json");
        assert_eq!(
            state_json,
            serde_json::json!({
                "type": "delegation_state", "id": 53,
                "enabled": false, "source": wire
            }),
            "the {wire} spelling is the one the app renders"
        );
        assert_eq!(
            serde_json::from_value::<DaemonMessage>(state_json).expect("back"),
            state
        );
    }

    // The write reply carries what the daemon stored, not an echo of the
    // request — the same stored-document rule the NOTE argues for
    // `AgentProfilesSet` — so the fields are part of the contract, not
    // decoration a rename could drop.
    let set_ok = DaemonMessage::DelegationSetOk {
        id: 54,
        enabled: true,
        source: DelegationSource::File,
    };
    assert_eq!(
        serde_json::to_value(&set_ok).expect("json"),
        serde_json::json!({
            "type": "delegation_set_ok", "id": 54,
            "enabled": true, "source": "file"
        })
    );

    // The push has no id: it answers no request, so it must never be
    // routed into the client's pending-request table.
    let changed = DaemonMessage::DelegationChanged {
        enabled: false,
        source: DelegationSource::Quarantined,
    };
    assert_eq!(
        serde_json::to_value(&changed).expect("json"),
        serde_json::json!({
            "type": "delegation_changed", "enabled": false,
            "source": "quarantined"
        })
    );

    // The audit and rate-limit sides: the read produces no row, the write
    // does.
    assert!(!ClientMessage::DelegationGet { id: 1 }.is_state_changing());
    assert!(ClientMessage::DelegationSet {
        id: 1,
        enabled: true
    }
    .is_state_changing());
    assert_eq!(
        ClientMessage::DelegationGet { id: 1 }.name(),
        "DelegationGet"
    );
    assert_eq!(
        ClientMessage::DelegationSet {
            id: 1,
            enabled: true
        }
        .name(),
        "DelegationSet"
    );
}

#[test]
fn provider_tools_are_camel_case_and_omitted_when_empty() {
    let mut row: ProviderInfo = serde_json::from_value(serde_json::json!({
        "id": "grok",
        "executable": "grok.exe",
        "acpAvailable": true,
        "authentication": "unknown"
    }))
    .expect("older row without tools");
    assert!(row.tools.is_empty(), "an absent key means no tools");
    assert!(serde_json::to_value(&row)
        .expect("json")
        .get("tools")
        .is_none());

    row.tools.push(ToolDescriptor {
        name: "devboule_list_agents".to_string(),
        description: "Lists live agent sessions.".to_string(),
    });
    let json = serde_json::to_value(&row).expect("json");
    assert_eq!(json["tools"][0]["name"], "devboule_list_agents");
    assert_eq!(
        json["tools"][0]["description"],
        "Lists live agent sessions."
    );
    assert_eq!(
        serde_json::from_value::<ProviderInfo>(json).expect("back"),
        row
    );
}

#[test]
fn providers_list_round_trips_with_camel_case_and_unknown_auth() {
    let request = ClientMessage::ProvidersList { id: 9 };
    let request_json = serde_json::to_value(&request).expect("json");
    assert_eq!(request_json["type"], "providers_list");
    assert_eq!(request_json["id"], 9);

    let reply = DaemonMessage::Providers {
        id: 9,
        providers: vec![ProviderInfo {
            id: "grok".to_string(),
            executable: r"C:\Users\gualt\AppData\Roaming\npm\grok.cmd".to_string(),
            acp_available: true,
            authentication: "unknown".to_string(),
            protocol: Some("acp".to_string()),
            origin: None,
            launch_args: None,
            pickable: None,
            installed_version: None,
            latest_version: None,
            agent_version: None,
            install_channel: None,
            installed: true,
            npm_package: None,
            tools: Vec::new(),
        }],
        unreadable_dirs: 2,
    };
    let encoded = serde_json::to_value(&reply).expect("json");
    assert_eq!(encoded["type"], "providers");
    assert_eq!(encoded["providers"][0]["id"], "grok");
    assert_eq!(encoded["providers"][0]["acpAvailable"], true);
    assert_eq!(encoded["providers"][0]["protocol"], "acp");
    assert_eq!(encoded["providers"][0]["authentication"], "unknown");
    assert_eq!(encoded["unreadableDirs"], 2);
    assert!(encoded["providers"][0].get("authenticated").is_none());
    assert_eq!(
        serde_json::from_value::<DaemonMessage>(encoded).expect("round trip"),
        reply
    );
}

#[test]
fn providers_refresh_round_trips_with_same_providers_shape() {
    let request = ClientMessage::ProvidersRefresh { id: 12 };
    let encoded = serde_json::to_value(&request).expect("json");
    assert_eq!(encoded["type"], "providers_refresh");
    assert_eq!(encoded["id"], 12);

    let reply = DaemonMessage::Providers {
        id: 12,
        providers: vec![
            ProviderInfo {
                id: "grok".to_string(),
                executable: "grok.exe".to_string(),
                acp_available: true,
                authentication: "unknown".to_string(),
                protocol: Some("acp".to_string()),
                origin: Some("user-binary".to_string()),
                launch_args: None,
                pickable: None,
                installed_version: Some("1.2.3".to_string()),
                latest_version: Some("1.2.4".to_string()),
                agent_version: Some("adapter-1".to_string()),
                install_channel: Some("native".to_string()),
                installed: true,
                npm_package: None,
                tools: Vec::new(),
            },
            ProviderInfo {
                id: "pi".to_string(),
                executable: "pi.exe".to_string(),
                acp_available: false,
                authentication: "unknown".to_string(),
                protocol: None,
                origin: Some("user-binary".to_string()),
                launch_args: None,
                pickable: None,
                installed_version: Some("0.1.0".to_string()),
                latest_version: None,
                agent_version: None,
                install_channel: Some("native".to_string()),
                installed: true,
                npm_package: None,
                tools: Vec::new(),
            },
        ],
        unreadable_dirs: 0,
    };
    let encoded = serde_json::to_value(&reply).expect("json");
    assert_eq!(encoded["providers"][0]["installedVersion"], "1.2.3");
    assert_eq!(encoded["providers"][0]["latestVersion"], "1.2.4");
    assert_eq!(encoded["providers"][0]["agentVersion"], "adapter-1");
    assert_eq!(encoded["providers"][0]["installChannel"], "native");
    assert_eq!(encoded["providers"][1]["installedVersion"], "0.1.0");
    assert!(encoded["providers"][1].get("latestVersion").is_none());
    assert!(encoded["providers"][1].get("agentVersion").is_none());
    assert_eq!(
        serde_json::from_value::<DaemonMessage>(encoded).expect("round trip"),
        reply
    );
}

#[test]
fn provider_origin_is_camel_case_on_the_wire() {
    let reply = DaemonMessage::Providers {
        id: 3,
        providers: vec![ProviderInfo {
            id: "codex-acp".to_string(),
            executable: "@agentclientprotocol/codex-acp@1.10.0".to_string(),
            acp_available: true,
            authentication: "unknown".to_string(),
            protocol: Some("acp".to_string()),
            origin: Some("npx-wrapper".to_string()),
            launch_args: None,
            pickable: None,
            installed_version: None,
            latest_version: None,
            agent_version: None,
            install_channel: None,
            installed: true,
            npm_package: None,
            tools: Vec::new(),
        }],
        unreadable_dirs: 0,
    };
    let encoded = serde_json::to_value(&reply).expect("json");
    assert_eq!(encoded["providers"][0]["origin"], "npx-wrapper");
    assert!(encoded["providers"][0].get("npx_wrapper").is_none());
    let native = ProviderInfo {
        id: "grok".to_string(),
        executable: r"C:\npm\grok.exe".to_string(),
        acp_available: true,
        authentication: "unknown".to_string(),
        protocol: Some("acp".to_string()),
        origin: Some("user-binary".to_string()),
        launch_args: None,
        pickable: None,
        installed_version: None,
        latest_version: None,
        agent_version: None,
        install_channel: None,
        installed: true,
        npm_package: None,
        tools: Vec::new(),
    };
    let native_json = serde_json::to_value(&native).expect("json");
    assert_eq!(native_json["origin"], "user-binary");
    assert_eq!(
        serde_json::from_value::<ProviderInfo>(native_json).expect("round trip"),
        native
    );
}

#[test]
fn provider_launch_args_and_pickable_are_optional_camel_case_fields() {
    let wrapper = ProviderInfo {
        id: "codex-acp".to_string(),
        executable: "@agentclientprotocol/codex-acp@1.10.0".to_string(),
        acp_available: true,
        authentication: "unknown".to_string(),
        protocol: Some("acp".to_string()),
        origin: Some("npx-wrapper".to_string()),
        launch_args: Some(vec!["--registry=https://evil".to_string()]),
        pickable: Some(false),
        installed_version: None,
        latest_version: None,
        agent_version: None,
        install_channel: None,
        installed: true,
        npm_package: None,
        tools: Vec::new(),
    };
    let encoded = serde_json::to_value(&wrapper).expect("json");
    assert_eq!(encoded["launchArgs"][0], "--registry=https://evil");
    assert_eq!(encoded["pickable"], false);
    assert_eq!(
        serde_json::from_value::<ProviderInfo>(encoded).expect("round trip"),
        wrapper
    );

    let native = ProviderInfo {
        launch_args: None,
        pickable: None,
        ..wrapper
    };
    let native_json = serde_json::to_value(native).expect("json");
    assert!(native_json.get("launchArgs").is_none());
    assert!(native_json.get("pickable").is_none());
}

#[test]
fn provider_update_request_and_reply_round_trip_with_camel_case_fields() {
    let request = ClientMessage::ProviderUpdate {
        id: 41,
        provider_id: "codex".to_string(),
    };
    let request_json = serde_json::to_value(&request).expect("json");
    assert_eq!(request_json["type"], "provider_update");
    assert_eq!(request_json["providerId"], "codex");

    let reply = DaemonMessage::ProviderUpdated {
        id: 41,
        ok: false,
        exit_code: Some(7),
        log: "npm output\nlast line".to_string(),
    };
    let reply_json = serde_json::to_value(&reply).expect("json");
    assert_eq!(reply_json["type"], "provider_updated");
    assert_eq!(reply_json["exitCode"], 7);
    assert_eq!(
        serde_json::from_value::<DaemonMessage>(reply_json).expect("round trip"),
        reply
    );

    let no_exit_code = serde_json::json!({
        "type": "provider_updated",
        "id": 42,
        "ok": false,
        "log": "npm was not found on PATH"
    });
    assert!(serde_json::to_value(DaemonMessage::ProviderUpdated {
        id: 42,
        ok: false,
        exit_code: None,
        log: "npm was not found on PATH".to_string(),
    })
    .expect("missing exit code json")
    .get("exitCode")
    .is_none());
    assert_eq!(
        serde_json::from_value::<DaemonMessage>(no_exit_code).expect("missing exit code"),
        DaemonMessage::ProviderUpdated {
            id: 42,
            ok: false,
            exit_code: None,
            log: "npm was not found on PATH".to_string(),
        }
    );
}

#[test]
fn provider_info_installed_false_is_emitted_and_missing_means_true() {
    let not_installed = ProviderInfo {
        id: "codex".to_string(),
        executable: String::new(),
        acp_available: false,
        authentication: "unknown".to_string(),
        protocol: None,
        origin: Some("user-binary".to_string()),
        launch_args: None,
        pickable: Some(false),
        installed_version: None,
        latest_version: Some("1.2.3".to_string()),
        agent_version: None,
        install_channel: Some("npm".to_string()),
        installed: false,
        npm_package: Some("@openai/codex".to_string()),
        tools: Vec::new(),
    };
    let encoded = serde_json::to_value(&not_installed).expect("json");
    assert_eq!(encoded["installed"], false);
    assert_eq!(encoded["npmPackage"], "@openai/codex");
    let installed_wire = serde_json::to_value(ProviderInfo {
        installed: true,
        ..not_installed.clone()
    })
    .expect("installed json");
    assert!(installed_wire.get("installed").is_none());

    let installed: ProviderInfo = serde_json::from_value(serde_json::json!({
        "id": "codex",
        "executable": "codex.exe",
        "acpAvailable": false,
        "authentication": "unknown"
    }))
    .expect("older provider row");
    assert!(installed.installed);
    assert_eq!(installed.npm_package, None);
}

#[test]
fn synthetic_provider_info_round_trips_installed_package_and_latest_version() {
    let synthetic = ProviderInfo {
        id: "qwen".to_string(),
        executable: String::new(),
        acp_available: false,
        authentication: "unknown".to_string(),
        protocol: None,
        origin: None,
        launch_args: None,
        pickable: Some(false),
        installed_version: None,
        latest_version: Some("0.23.0".to_string()),
        agent_version: None,
        install_channel: Some("npm".to_string()),
        installed: false,
        npm_package: Some("@qwen-code/qwen-code".to_string()),
        tools: Vec::new(),
    };
    let encoded = serde_json::to_value(&synthetic).expect("synthetic json");
    assert_eq!(encoded["installed"], false);
    assert_eq!(encoded["npmPackage"], "@qwen-code/qwen-code");
    assert_eq!(encoded["latestVersion"], "0.23.0");
    assert_eq!(
        serde_json::from_value::<ProviderInfo>(encoded).expect("synthetic round trip"),
        synthetic
    );
}

/// `display_name` is one field of `session_create`, camelCase on the wire and
/// optional: a frame that omits it is still a valid create (S5-09).
#[test]
fn session_create_display_name_is_camel_case_and_optional() {
    let named = ClientMessage::SessionCreate {
        id: 12,
        workspace_id: None,
        kind: SessionKind::Claude,
        provider: None,
        mode: None,
        display_name: Some("worker".to_string()),
        idempotency_key: None,
    };
    let value = serde_json::to_value(&named).expect("json");
    assert_eq!(value["displayName"], "worker");
    assert!(value.get("display_name").is_none());

    let bare = serde_json::json!({
        "type": "session_create",
        "id": 13,
        "workspaceId": null,
        "kind": "claude",
    });
    let decoded: ClientMessage = serde_json::from_value(bare).expect("older frame");
    assert!(matches!(
        decoded,
        ClientMessage::SessionCreate {
            display_name: None,
            ..
        }
    ));

    // There is no `createdBy` on a create frame: the parent is the daemon's
    // fact and a client that sends one is not describing itself as a child.
    let claiming = serde_json::json!({
        "type": "session_create",
        "id": 14,
        "workspaceId": null,
        "kind": "claude",
        "displayName": "worker",
        "createdBy": "s.parent.1",
    });
    let decoded: ClientMessage = serde_json::from_value(claiming).expect("frame with a claim");
    let ClientMessage::SessionCreate { display_name, .. } = decoded else {
        panic!("expected a create frame");
    };
    assert_eq!(display_name.as_deref(), Some("worker"));
}

/// The bound, its trim, and the fact that no refusal echoes the name.
#[test]
fn a_display_name_is_trimmed_then_capped_at_sixty_characters() {
    assert_eq!(
        validate_display_name("  worker  "),
        Ok("worker".to_string()),
        "the value is trimmed before it is judged, and the trimmed one is kept"
    );
    assert!(validate_display_name("").is_err());
    assert!(validate_display_name("   \t ").is_err());
    let sixty = "worker".repeat(10);
    assert_eq!(sixty.chars().count(), crate::MAX_DISPLAY_NAME_CHARS);
    assert_eq!(validate_display_name(&sixty), Ok(sixty.clone()));
    let sixty_one = format!("{sixty}!");
    let error = validate_display_name(&sixty_one).expect_err("61 characters");
    assert!(
        error.contains("61") && error.contains("60"),
        "the sentence states both numbers: {error}"
    );
    assert!(
        !error.contains("worker"),
        "the refusal must not echo the name back: {error}"
    );
    // Counted in characters, not bytes: sixty accented characters fit.
    let accented = "è".repeat(crate::MAX_DISPLAY_NAME_CHARS);
    assert!(accented.len() > crate::MAX_DISPLAY_NAME_CHARS);
    assert_eq!(validate_display_name(&accented), Ok(accented));
}

/// The workspace git-status frame and its reply, pinned to the exact words
/// TypeScript reads (`src/types/ipc.ts`): the `type` tags, the camelCase keys,
/// and the six `status` values spelled in `snake_case`.
#[test]
fn workspace_git_status_round_trips_with_its_wire_words() {
    let status = WorkspaceGitStatus {
        is_git: true,
        dirty: true,
        branch: Some("main".to_string()),
        totals: WorkspaceGitTotals {
            additions: 3,
            deletions: 1,
        },
        rows: vec![WorkspaceGitRow {
            path: "src/lib.rs".to_string(),
            renamed_from: None,
            additions: 3,
            deletions: 1,
            status: WorkspaceGitFileStatus::Modified,
            capped: false,
        }],
        error: None,
    };
    let reply = DaemonMessage::WorkspaceGit { id: 7, status };
    let json = serde_json::to_string(&reply).expect("serialize");
    for needle in [
        "\"type\":\"workspace_git\"",
        "\"isGit\":true",
        "\"dirty\":true",
        "\"branch\":\"main\"",
        "\"totals\":{\"additions\":3,\"deletions\":1}",
        "\"status\":\"modified\"",
        "\"capped\":false",
        "\"error\":null",
    ] {
        assert!(json.contains(needle), "{needle} missing from {json}");
    }
    assert_eq!(
        serde_json::from_str::<DaemonMessage>(&json).expect("parse"),
        reply
    );

    let request = ClientMessage::WorkspaceGitStatus {
        id: 7,
        workspace_id: "ws.1".to_string(),
    };
    let json = serde_json::to_string(&request).expect("serialize");
    assert!(json.contains("\"type\":\"workspace_git_status\""), "{json}");
    assert!(json.contains("\"workspaceId\":\"ws.1\""), "{json}");
    assert_eq!(
        serde_json::from_str::<ClientMessage>(&json).expect("parse"),
        request
    );
    assert_eq!(request.name(), "WorkspaceGitStatus");
    assert!(!request.is_state_changing(), "a read writes nothing");
    assert_eq!(request.request_id(), Some(7));

    // An absent branch and a present error are both `null` on the wire, never
    // an omitted key: the TypeScript side types them as nullable, and a key
    // TypeScript cannot see is a crash it cannot predict.
    let refused = WorkspaceGitStatus {
        branch: None,
        error: Some("the workspace folder is not a directory".to_string()),
        ..reply_status(&reply)
    };
    let json = serde_json::to_string(&refused).expect("serialize");
    assert!(json.contains("\"branch\":null"), "{json}");
    assert!(json.contains("\"error\":\"the workspace folder"), "{json}");

    for (word, spelled) in [
        (WorkspaceGitFileStatus::Modified, "\"modified\""),
        (WorkspaceGitFileStatus::Added, "\"added\""),
        (WorkspaceGitFileStatus::Deleted, "\"deleted\""),
        (WorkspaceGitFileStatus::Renamed, "\"renamed\""),
        (WorkspaceGitFileStatus::Untracked, "\"untracked\""),
        (WorkspaceGitFileStatus::Conflicted, "\"conflicted\""),
    ] {
        assert_eq!(serde_json::to_string(&word).expect("serialize"), spelled);
    }
}

fn reply_status(message: &DaemonMessage) -> WorkspaceGitStatus {
    match message {
        DaemonMessage::WorkspaceGit { status, .. } => status.clone(),
        other => panic!("not a workspace git reply: {other:?}"),
    }
}

/// The workspace git-diff frame and its reply, pinned to the exact words
/// TypeScript reads (`src/types/ipc.ts`): the `type` tags, the camelCase
/// keys, the four `status` words and the four `kind` words in `snake_case`,
/// and `error` travelling as `null` rather than being omitted.
#[test]
fn workspace_git_diff_round_trips_with_its_wire_words() {
    let file = WorkspaceGitFileDiff {
        path: "src/lib.rs".to_string(),
        is_new: false,
        is_deleted: true,
        additions: 0,
        deletions: 2,
        lines: vec![
            WorkspaceGitDiffLine {
                kind: WorkspaceGitDiffLineKind::Header,
                text: "@@ -1,3 +0,0 @@".to_string(),
            },
            WorkspaceGitDiffLine {
                kind: WorkspaceGitDiffLineKind::Remove,
                text: "gone".to_string(),
            },
        ],
        status: WorkspaceGitDiffStatus::Ok,
        error: None,
    };
    let reply = DaemonMessage::WorkspaceGitFile {
        id: 9,
        file: file.clone(),
    };
    let json = serde_json::to_string(&reply).expect("serialize");
    for needle in [
        "\"type\":\"workspace_git_file\"",
        "\"path\":\"src/lib.rs\"",
        "\"isDeleted\":true",
        "\"isNew\":false",
        "\"deletions\":2",
        "\"kind\":\"header\"",
        "\"text\":\"@@ -1,3 +0,0 @@\"",
        "\"kind\":\"remove\"",
        "\"status\":\"ok\"",
        "\"error\":null",
    ] {
        assert!(json.contains(needle), "{needle} missing from {json}");
    }
    assert_eq!(
        serde_json::from_str::<DaemonMessage>(&json).expect("parse"),
        reply
    );

    let request = ClientMessage::WorkspaceGitDiff {
        id: 9,
        workspace_id: "ws.1".to_string(),
        path: "src/lib.rs".to_string(),
    };
    let json = serde_json::to_string(&request).expect("serialize");
    assert!(json.contains("\"type\":\"workspace_git_diff\""), "{json}");
    assert!(json.contains("\"workspaceId\":\"ws.1\""), "{json}");
    assert!(json.contains("\"path\":\"src/lib.rs\""), "{json}");
    assert_eq!(
        serde_json::from_str::<ClientMessage>(&json).expect("parse"),
        request
    );
    assert_eq!(request.name(), "WorkspaceGitDiff");
    assert!(!request.is_state_changing(), "a read writes nothing");
    assert_eq!(request.request_id(), Some(9));

    for (status, spelled) in [
        (WorkspaceGitDiffStatus::Ok, "\"ok\""),
        (WorkspaceGitDiffStatus::Binary, "\"binary\""),
        (WorkspaceGitDiffStatus::TooLarge, "\"too_large\""),
        (WorkspaceGitDiffStatus::Error, "\"error\""),
    ] {
        assert_eq!(serde_json::to_string(&status).expect("serialize"), spelled);
    }
    for (kind, spelled) in [
        (WorkspaceGitDiffLineKind::Add, "\"add\""),
        (WorkspaceGitDiffLineKind::Remove, "\"remove\""),
        (WorkspaceGitDiffLineKind::Context, "\"context\""),
        (WorkspaceGitDiffLineKind::Header, "\"header\""),
    ] {
        assert_eq!(serde_json::to_string(&kind).expect("serialize"), spelled);
    }

    // A refusal and a cap both carry their sentence; an omitted `error` key
    // would be a field TypeScript types as nullable and cannot see.
    let refused = WorkspaceGitFileDiff {
        status: WorkspaceGitDiffStatus::Error,
        error: Some("the requested path is outside the workspace folder".to_string()),
        lines: Vec::new(),
        ..file
    };
    let json = serde_json::to_string(&refused).expect("serialize");
    assert!(
        json.contains("\"status\":\"error\""),
        "the refusal word, spelled: {json}"
    );
    assert!(
        json.contains("\"error\":\"the requested path"),
        "the sentence travels: {json}"
    );
}

/// The workspace files-list frame and its reply, pinned to the exact words
/// TypeScript reads (`src/types/ipc.ts`): the `type` tags, the camelCase
/// keys, the two `kind` values spelled in `snake_case`, a folder's `size`
/// travelling as `null` rather than being omitted, and the empty `path` —
/// the folder itself — travelling as an empty string rather than vanishing.
#[test]
fn workspace_files_list_round_trips_with_its_wire_words() {
    let directory = WorkspaceDirectory {
        path: "crates".to_string(),
        entries: vec![
            WorkspaceFileEntry {
                path: "crates/devboule-daemon".to_string(),
                name: "devboule-daemon".to_string(),
                kind: WorkspaceFileKind::Dir,
                size: None,
            },
            WorkspaceFileEntry {
                path: "crates/README.md".to_string(),
                name: "README.md".to_string(),
                kind: WorkspaceFileKind::File,
                size: Some(512),
            },
        ],
        capped: false,
        skipped: 1,
        error: None,
    };
    let reply = DaemonMessage::WorkspaceFiles { id: 11, directory };
    let json = serde_json::to_string(&reply).expect("serialize");
    for needle in [
        "\"type\":\"workspace_files\"",
        "\"path\":\"crates\"",
        "\"name\":\"devboule-daemon\"",
        "\"kind\":\"dir\"",
        "\"kind\":\"file\"",
        "\"size\":null",
        "\"size\":512",
        "\"capped\":false",
        "\"skipped\":1",
        "\"error\":null",
    ] {
        assert!(json.contains(needle), "{needle} missing from {json}");
    }
    assert_eq!(
        serde_json::from_str::<DaemonMessage>(&json).expect("parse"),
        reply
    );

    let request = ClientMessage::WorkspaceFilesList {
        id: 11,
        workspace_id: "ws.1".to_string(),
        path: "crates".to_string(),
    };
    let json = serde_json::to_string(&request).expect("serialize");
    assert!(json.contains("\"type\":\"workspace_files_list\""), "{json}");
    assert!(json.contains("\"workspaceId\":\"ws.1\""), "{json}");
    assert!(json.contains("\"path\":\"crates\""), "{json}");
    assert_eq!(
        serde_json::from_str::<ClientMessage>(&json).expect("parse"),
        request
    );
    assert_eq!(request.name(), "WorkspaceFilesList");
    assert!(!request.is_state_changing(), "a read writes nothing");
    assert_eq!(request.request_id(), Some(11));

    let root = ClientMessage::WorkspaceFilesList {
        id: 12,
        workspace_id: "ws.1".to_string(),
        path: String::new(),
    };
    let json = serde_json::to_string(&root).expect("serialize");
    assert!(
        json.contains("\"path\":\"\""),
        "the folder itself travels as an empty path: {json}"
    );
    assert_eq!(
        serde_json::from_str::<ClientMessage>(&json).expect("parse"),
        root
    );

    for (kind, spelled) in [
        (WorkspaceFileKind::Dir, "\"dir\""),
        (WorkspaceFileKind::File, "\"file\""),
    ] {
        assert_eq!(serde_json::to_string(&kind).expect("serialize"), spelled);
    }

    // A refusal carries its sentence and no entries — an omitted `error`
    // key would be a field TypeScript types as nullable and cannot see.
    let refused = WorkspaceDirectory {
        path: "../outside".to_string(),
        entries: Vec::new(),
        capped: false,
        skipped: 0,
        error: Some("the requested path is outside the workspace folder".to_string()),
    };
    let json = serde_json::to_string(&refused).expect("serialize");
    assert!(json.contains("\"entries\":[]"), "{json}");
    assert!(
        json.contains("\"error\":\"the requested path"),
        "the sentence travels: {json}"
    );
}

/// The file-read frame and its reply, pinned to the exact words TypeScript
/// reads (`src/types/ipc.ts`): the `type` tags, the camelCase keys, the four
/// status words spelled in `snake_case`, `content`/`size`/`kind` travelling
/// as explicit `null` rather than vanishing, and — the reason this frame is
/// allowed on the wire at all — `is_state_changing` staying `false`: a read
/// writes nothing.
#[test]
fn workspace_file_read_round_trips_with_its_wire_words() {
    let reply = DaemonMessage::WorkspaceFileContent {
        id: 13,
        file: WorkspaceFileContent {
            status: WorkspaceFileContentStatus::Ok,
            kind: Some(WorkspaceFileContentKind::Text),
            content: Some("hello\n".to_string()),
            size: Some(6),
            modified_at: Some(1_758_000_000_000),
            error: None,
            from_line: Some(1),
            lines: Some(1),
            has_more: Some(false),
            truncated: Some(false),
            note: None,
        },
    };
    let json = serde_json::to_string(&reply).expect("serialize");
    for needle in [
        "\"type\":\"workspace_file_content\"",
        "\"status\":\"ok\"",
        "\"kind\":\"text\"",
        "\"content\":\"hello\\n\"",
        "\"modifiedAt\":1758000000000",
        "\"error\":null",
        "\"fromLine\":1",
        "\"lines\":1",
        "\"hasMore\":false",
        "\"truncated\":false",
        "\"note\":null",
    ] {
        assert!(json.contains(needle), "{needle} missing from {json}");
    }
    assert_eq!(
        serde_json::from_str::<DaemonMessage>(&json).expect("parse"),
        reply
    );

    let request = ClientMessage::WorkspaceFileRead {
        id: 13,
        workspace_id: "ws.1".to_string(),
        path: "README.md".to_string(),
        from_line: None,
        line_count: None,
    };
    let json = serde_json::to_string(&request).expect("serialize");
    assert!(json.contains("\"type\":\"workspace_file_read\""), "{json}");
    assert!(json.contains("\"workspaceId\":\"ws.1\""), "{json}");
    assert!(json.contains("\"path\":\"README.md\""), "{json}");
    // No window named is the first window, and the frame stays the one
    // older journals hold — the window keys join it only when a caller
    // asks for a later one.
    assert!(!json.contains("fromLine"), "{json}");
    assert!(!json.contains("lineCount"), "{json}");
    assert_eq!(
        serde_json::from_str::<ClientMessage>(&json).expect("parse"),
        request
    );
    let windowed = ClientMessage::WorkspaceFileRead {
        id: 13,
        workspace_id: "ws.1".to_string(),
        path: "README.md".to_string(),
        from_line: Some(2001),
        line_count: Some(5000),
    };
    let json = serde_json::to_string(&windowed).expect("serialize");
    assert!(json.contains("\"fromLine\":2001"), "{json}");
    assert!(json.contains("\"lineCount\":5000"), "{json}");
    assert_eq!(
        serde_json::from_str::<ClientMessage>(&json).expect("parse"),
        windowed
    );
    assert_eq!(request.name(), "WorkspaceFileRead");
    assert!(!request.is_state_changing(), "a read writes nothing");
    assert_eq!(request.request_id(), Some(13));

    for (status, spelled) in [
        (WorkspaceFileContentStatus::Ok, "\"ok\""),
        (WorkspaceFileContentStatus::TooLarge, "\"too_large\""),
        (WorkspaceFileContentStatus::Binary, "\"binary\""),
        (WorkspaceFileContentStatus::Refused, "\"refused\""),
    ] {
        assert_eq!(serde_json::to_string(&status).expect("serialize"), spelled);
    }

    // A withholding carries no content and a refusal no size: both travel
    // as explicit `null`, because an omitted key is a field TypeScript
    // types as nullable and cannot see.
    let refused = DaemonMessage::WorkspaceFileContent {
        id: 14,
        file: WorkspaceFileContent {
            status: WorkspaceFileContentStatus::Refused,
            kind: None,
            content: None,
            size: None,
            modified_at: None,
            error: Some("the requested path is outside the workspace folder".to_string()),
            from_line: None,
            lines: None,
            has_more: None,
            truncated: None,
            note: None,
        },
    };
    let json = serde_json::to_string(&refused).expect("serialize");
    for needle in [
        "\"status\":\"refused\"",
        "\"kind\":null",
        "\"content\":null",
        "\"size\":null",
        "\"fromLine\":null",
        "\"error\":\"the requested path",
    ] {
        assert!(json.contains(needle), "{needle} missing from {json}");
    }
}

/// The two write frames of the Files panel and their replies, pinned the
/// same way the read's are: the `type` tags, the camelCase keys (including
/// the optional `idempotencyKey`, omitted when absent), `newPath` and
/// `error` travelling as explicit `null` — and, the reason these two frames
/// are the first workspace ones allowed to write at all,
/// `is_state_changing` being `true`: a rename and a duplicate are audited.
#[test]
fn workspace_file_writes_round_trip_with_their_wire_words() {
    let renamed = DaemonMessage::WorkspaceFileRenamed {
        id: 21,
        change: WorkspaceFileMutation {
            new_path: Some("docs/guide.md".to_string()),
            error: None,
        },
    };
    let json = serde_json::to_string(&renamed).expect("serialize");
    for needle in [
        "\"type\":\"workspace_file_renamed\"",
        "\"newPath\":\"docs/guide.md\"",
        "\"error\":null",
    ] {
        assert!(json.contains(needle), "{needle} missing from {json}");
    }
    assert_eq!(
        serde_json::from_str::<DaemonMessage>(&json).expect("parse"),
        renamed
    );

    let duplicated = DaemonMessage::WorkspaceFileDuplicated {
        id: 22,
        change: WorkspaceFileMutation {
            new_path: None,
            error: Some("an entry with that new name already exists".to_string()),
        },
    };
    let json = serde_json::to_string(&duplicated).expect("serialize");
    for needle in [
        "\"type\":\"workspace_file_duplicated\"",
        "\"newPath\":null",
        "\"error\":\"an entry with that new name",
    ] {
        assert!(json.contains(needle), "{needle} missing from {json}");
    }
    assert_eq!(
        serde_json::from_str::<DaemonMessage>(&json).expect("parse"),
        duplicated
    );

    let request = ClientMessage::WorkspaceFileRename {
        id: 21,
        workspace_id: "ws.1".to_string(),
        path: "docs/guide.txt".to_string(),
        name: "guide.md".to_string(),
        idempotency_key: Some("k-21".to_string()),
    };
    let json = serde_json::to_string(&request).expect("serialize");
    assert!(
        json.contains("\"type\":\"workspace_file_rename\""),
        "{json}"
    );
    assert!(json.contains("\"name\":\"guide.md\""), "{json}");
    assert!(json.contains("\"idempotencyKey\":\"k-21\""), "{json}");
    assert_eq!(
        serde_json::from_str::<ClientMessage>(&json).expect("parse"),
        request
    );
    assert_eq!(request.name(), "WorkspaceFileRename");
    assert!(request.is_state_changing(), "a write is audited");
    assert_eq!(request.request_id(), Some(21));
    assert_eq!(request.idempotency_key(), Some("k-21"));

    let duplicate = ClientMessage::WorkspaceFileDuplicate {
        id: 22,
        workspace_id: "ws.1".to_string(),
        path: "README.md".to_string(),
        idempotency_key: None,
    };
    let json = serde_json::to_string(&duplicate).expect("serialize");
    assert!(
        json.contains("\"type\":\"workspace_file_duplicate\""),
        "{json}"
    );
    // Absent, not `null`: the field is typed optional in TypeScript, and an
    // omitted key is the only spelling `skip_serializing_if` ever produces.
    assert!(!json.contains("idempotencyKey"), "{json}");
    assert_eq!(
        serde_json::from_str::<ClientMessage>(&json).expect("parse"),
        duplicate
    );
    assert_eq!(duplicate.name(), "WorkspaceFileDuplicate");
    assert!(duplicate.is_state_changing(), "a write is audited");
    assert_eq!(duplicate.request_id(), Some(22));
    assert_eq!(duplicate.idempotency_key(), None);
}

/// The third write frame — the one that loses data — pinned the same way:
/// the `type` tags, the camelCase `idempotencyKey`, and the success shape
/// unique to this act, `newPath` **and** `error` travelling as explicit
/// `null` because there is no spelling left to name. The refusal carries
/// the sentence, never a path.
#[test]
fn workspace_file_delete_round_trips_with_its_wire_words() {
    let deleted = DaemonMessage::WorkspaceFileDeleted {
        id: 23,
        change: WorkspaceFileMutation {
            new_path: None,
            error: None,
        },
    };
    let json = serde_json::to_string(&deleted).expect("serialize");
    for needle in [
        "\"type\":\"workspace_file_deleted\"",
        "\"newPath\":null",
        "\"error\":null",
    ] {
        assert!(json.contains(needle), "{needle} missing from {json}");
    }
    assert_eq!(
        serde_json::from_str::<DaemonMessage>(&json).expect("parse"),
        deleted
    );

    let refused = DaemonMessage::WorkspaceFileDeleted {
        id: 24,
        change: WorkspaceFileMutation {
            new_path: None,
            error: Some(
                "the workspace's own folder cannot be renamed, duplicated or deleted".to_string(),
            ),
        },
    };
    let json = serde_json::to_string(&refused).expect("serialize");
    for needle in [
        "\"type\":\"workspace_file_deleted\"",
        "\"newPath\":null",
        "\"error\":\"the workspace's own folder",
    ] {
        assert!(json.contains(needle), "{needle} missing from {json}");
    }
    assert_eq!(
        serde_json::from_str::<DaemonMessage>(&json).expect("parse"),
        refused
    );

    let request = ClientMessage::WorkspaceFileDelete {
        id: 23,
        workspace_id: "ws.1".to_string(),
        path: "old-notes.txt".to_string(),
        idempotency_key: Some("k-23".to_string()),
    };
    let json = serde_json::to_string(&request).expect("serialize");
    for needle in [
        "\"type\":\"workspace_file_delete\"",
        "\"path\":\"old-notes.txt\"",
        "\"idempotencyKey\":\"k-23\"",
    ] {
        assert!(json.contains(needle), "{needle} missing from {json}");
    }
    assert_eq!(
        serde_json::from_str::<ClientMessage>(&json).expect("parse"),
        request
    );
    assert_eq!(request.name(), "WorkspaceFileDelete");
    assert!(request.is_state_changing(), "a delete is audited");
    assert_eq!(request.request_id(), Some(23));
    assert_eq!(request.idempotency_key(), Some("k-23"));
}

/// The preview's two frames, pinned the way the read's and the writes' are:
/// the `type` tags, the camelCase keys, `path`/`size`/`modifiedAt`/`error`
/// travelling as explicit `null` on the refusal — and both frames on the
/// state-changing side, because a stage creates a copy in the runtime
/// directory and an unstage deletes it: each earns an audit row, and
/// neither carries an idempotency key (a re-staged copy is the same copy,
/// a re-run unstage deletes the same folder).
#[test]
fn workspace_file_preview_frames_round_trip_with_their_wire_words() {
    let staged = DaemonMessage::WorkspaceFilePreviewStaged {
        id: 31,
        staged: WorkspaceFilePreview {
            status: WorkspaceFilePreviewStatus::Ok,
            path: Some(r"C:\Users\u\AppData\Local\Devboule\previews\ab12.png".to_string()),
            size: Some(8),
            modified_at: Some(1_758_000_000_000),
            error: None,
        },
    };
    let json = serde_json::to_string(&staged).expect("serialize");
    for needle in [
        "\"type\":\"workspace_file_preview_staged\"",
        "\"status\":\"ok\"",
        "\"path\":\"C:\\\\Users\\\\u",
        "\"modifiedAt\":1758000000000",
        "\"error\":null",
    ] {
        assert!(json.contains(needle), "{needle} missing from {json}");
    }
    assert_eq!(
        serde_json::from_str::<DaemonMessage>(&json).expect("parse"),
        staged
    );

    // The refusal half: nothing travels but the sentence, and every other
    // field is an explicit `null` — a refusal claims nothing about a file
    // it never copied.
    let refused = DaemonMessage::WorkspaceFilePreviewStaged {
        id: 32,
        staged: WorkspaceFilePreview {
            status: WorkspaceFilePreviewStatus::Refused,
            path: None,
            size: None,
            modified_at: None,
            error: Some("the requested path is outside the workspace folder".to_string()),
        },
    };
    let json = serde_json::to_string(&refused).expect("serialize");
    for needle in [
        "\"status\":\"refused\"",
        "\"path\":null",
        "\"size\":null",
        "\"modifiedAt\":null",
        "\"error\":\"the requested path",
    ] {
        assert!(json.contains(needle), "{needle} missing from {json}");
    }
    assert_eq!(
        serde_json::from_str::<DaemonMessage>(&json).expect("parse"),
        refused
    );

    let stage = ClientMessage::WorkspaceFilePreviewStage {
        id: 31,
        workspace_id: "ws.1".to_string(),
        path: "docs/shot.png".to_string(),
    };
    let json = serde_json::to_string(&stage).expect("serialize");
    assert!(
        json.contains("\"type\":\"workspace_file_preview_stage\""),
        "{json}"
    );
    assert!(json.contains("\"workspaceId\":\"ws.1\""), "{json}");
    assert!(json.contains("\"path\":\"docs/shot.png\""), "{json}");
    assert_eq!(
        serde_json::from_str::<ClientMessage>(&json).expect("parse"),
        stage
    );
    assert_eq!(stage.name(), "WorkspaceFilePreviewStage");
    assert!(stage.is_state_changing(), "a stage writes a copy to disk");
    assert_eq!(stage.request_id(), Some(31));
    assert_eq!(stage.idempotency_key(), None);

    let unstage = ClientMessage::WorkspaceFilePreviewUnstage { id: 32 };
    let json = serde_json::to_string(&unstage).expect("serialize");
    assert!(
        json.contains("\"type\":\"workspace_file_preview_unstage\""),
        "{json}"
    );
    assert_eq!(
        serde_json::from_str::<ClientMessage>(&json).expect("parse"),
        unstage
    );
    assert_eq!(unstage.name(), "WorkspaceFilePreviewUnstage");
    assert!(
        unstage.is_state_changing(),
        "an unstage deletes a copy from disk"
    );
    assert_eq!(unstage.request_id(), Some(32));
    assert_eq!(unstage.idempotency_key(), None);
}

/// The request trace names a command through `name()`, which is a
/// `&'static str` constant — never through `Debug`, whose rendering carries
/// the payload (a prompt's text) inside.
#[test]
fn workspace_git_write_frames_round_trip_with_their_wire_words() {
    // The four git writes and their one shared reply: the wire words
    // (snake_case variants, camelCase fields), the closed-match answers
    // that name and audit them, and the request/idempotency arms the
    // protocol pins for every frame.
    let frames = [
        ClientMessage::WorkspaceGitStage {
            id: 7,
            workspace_id: "ws.1".to_string(),
            paths: vec!["-f".to_string(), "a[1].txt".to_string()],
            idempotency_key: Some("k-7".to_string()),
        },
        ClientMessage::WorkspaceGitUnstage {
            id: 8,
            workspace_id: "ws.1".to_string(),
            paths: vec!["src/lib.rs".to_string()],
            idempotency_key: None,
        },
        ClientMessage::WorkspaceGitDiscard {
            id: 9,
            workspace_id: "ws.1".to_string(),
            paths: vec!["notes/todo.md".to_string()],
            idempotency_key: None,
        },
        ClientMessage::WorkspaceGitCommit {
            id: 10,
            workspace_id: "ws.1".to_string(),
            message: "say what changed".to_string(),
            idempotency_key: None,
        },
    ];
    for (index, frame) in frames.iter().enumerate() {
        let json = serde_json::to_string(frame).expect("serialize");
        let variant = [
            "workspace_git_stage",
            "workspace_git_unstage",
            "workspace_git_discard",
            "workspace_git_commit",
        ][index];
        assert!(
            json.contains(&format!("\"type\":\"{variant}\"")),
            "{variant}: {json}"
        );
        assert!(json.contains("\"workspaceId\":\"ws.1\""), "{json}");
        assert_eq!(
            serde_json::from_str::<ClientMessage>(&json).expect("parse"),
            *frame,
            "{variant}"
        );
        // The four ids are 7, 8, 9, 10 in order — a literal the assertion
        // can fail against (comparing the value with itself would be a
        // tautology, which is what this line was).
        assert_eq!(frame.request_id(), Some(7 + index as u64));
        assert!(frame.is_state_changing(), "{variant}: a write is audited");
        assert_eq!(
            frame.idempotency_key(),
            if index == 0 { Some("k-7") } else { None },
            "{variant}"
        );
    }
    assert_eq!(frames[0].name(), "WorkspaceGitStage");
    assert_eq!(frames[1].name(), "WorkspaceGitUnstage");
    assert_eq!(frames[2].name(), "WorkspaceGitDiscard");
    assert_eq!(frames[3].name(), "WorkspaceGitCommit");

    // The one reply: null is landed, a sentence is the refusal — and the
    // reply itself carries no path (the message is the guard; `error`
    // does not pass the redaction seam).
    let landed = DaemonMessage::WorkspaceGitWrite {
        id: 11,
        error: None,
    };
    let refused = DaemonMessage::WorkspaceGitWrite {
        id: 12,
        error: Some(
            "another git process is using this repository; try again in a moment".to_string(),
        ),
    };
    for reply in [landed, refused] {
        let json = serde_json::to_string(&reply).expect("serialize");
        assert!(json.contains("\"type\":\"workspace_git_write\""), "{json}");
        assert_eq!(
            serde_json::from_str::<DaemonMessage>(&json).expect("parse"),
            reply
        );
    }
}

#[test]
fn trace_name_is_a_static_constant_and_never_carries_the_payload() {
    let text = "TRACE-SENTINEL-8f31 the prompt body must not be logged";
    let message = ClientMessage::AgentMessageSend {
        id: 9,
        from_session: "session-a".to_string(),
        to_session: "session-b".to_string(),
        text: text.to_string(),
        idempotency_key: None,
    };
    let name: &'static str = message.name();
    assert_eq!(name, "AgentMessageSend");
    assert!(!name.contains("TRACE-SENTINEL"), "{name}");
    assert!(!name.contains("session-a"), "{name}");
    // Debug is exactly what the trace must never reach for: the payload is in it.
    let debug = format!("{message:?}");
    assert!(
        debug.contains("TRACE-SENTINEL"),
        "the payload lives inside Debug: {debug}"
    );
    assert_ne!(name, debug);

    assert_eq!(ClientMessage::SessionsList { id: 1 }.name(), "SessionsList");
    assert_eq!(ClientMessage::Ping { id: 1 }.name(), "Ping");
}

#[test]
fn the_send_reply_carries_whether_a_turn_began() {
    // The out-of-band disposition: a turn began, or one did not. The variant
    // survives the wire verbatim so a receipt can replay it under a retry id.
    for turn_active in [true, false] {
        let reply = DaemonMessage::SessionSend { id: 7, turn_active };
        let wire = serde_json::to_value(&reply).expect("json");
        assert_eq!(wire["type"], "session_send");
        assert_eq!(wire["turnActive"], turn_active);
        assert_eq!(
            serde_json::from_value::<DaemonMessage>(wire).expect("parse"),
            reply
        );
    }
}
