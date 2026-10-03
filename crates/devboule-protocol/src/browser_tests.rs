//! The browser-host wire: that every new frame round-trips in the camelCase
//! shape `src/types/ipc.ts` declares, that the one error enum matches its
//! frontend union, and that a payload at the cap still fits one frame.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::json;

use super::*;
use crate::{ClientMessage, DaemonMessage, ErrorCode, ErrorDetails, WireError, MAX_FRAME_BYTES};

const EVERY_CODE: [(BrowserErrorCode, &str); 9] = [
    (BrowserErrorCode::NoHost, "browser_no_host"),
    (BrowserErrorCode::Timeout, "browser_timeout"),
    (BrowserErrorCode::Busy, "browser_busy"),
    (BrowserErrorCode::ResultTooLarge, "browser_result_too_large"),
    (BrowserErrorCode::ArgsTooLarge, "browser_args_too_large"),
    (BrowserErrorCode::HostError, "browser_host_error"),
    (
        BrowserErrorCode::UnsupportedCommand,
        "browser_unsupported_command",
    ),
    (
        BrowserErrorCode::OwnerUnavailable,
        "browser_owner_unavailable",
    ),
    (BrowserErrorCode::TabNotFound, "browser_tab_not_found"),
];

fn round_trip<T>(value: &T) -> serde_json::Value
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let wire = serde_json::to_value(value).expect("serialize");
    let back: T = serde_json::from_value(wire.clone()).expect("deserialize");
    assert_eq!(&back, value, "the frame must survive its own wire shape");
    wire
}

#[test]
fn the_error_enum_serializes_as_its_wire_names() {
    for (code, name) in EVERY_CODE {
        assert_eq!(serde_json::to_value(code).expect("json"), name);
        assert_eq!(
            code.as_str(),
            name,
            "the tool error text spells it the same"
        );
    }
}

#[test]
fn only_the_conditions_that_clear_themselves_are_retryable() {
    for (code, _) in EVERY_CODE {
        let error = BrowserError::daemon(code, "x");
        let expected = matches!(
            code,
            BrowserErrorCode::NoHost | BrowserErrorCode::Timeout | BrowserErrorCode::Busy
        );
        assert_eq!(error.retryable, expected, "{code:?}");
    }
}

#[test]
fn browser_error_code_matches_frontend_union() {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("src");
    path.push("types");
    path.push("ipc.ts");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    const MARKER: &str = "export type BrowserErrorCode";
    let after = &source[source.find(MARKER).expect("BrowserErrorCode alias") + MARKER.len()..];
    let body = &after[after.find('=').expect("=") + 1..];
    let body = &body[..body.find(';').expect(";")];
    let ts_names: BTreeSet<String> = body
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect();
    let rust_names: BTreeSet<String> = EVERY_CODE
        .iter()
        .map(|(_, name)| (*name).to_owned())
        .collect();
    assert_eq!(
        rust_names, ts_names,
        "BrowserErrorCode serde names and the union in src/types/ipc.ts drifted"
    );
}

#[test]
fn register_and_unregister_use_camel_case_fields() {
    let register = ClientMessage::BrowserHostRegister {
        id: 1,
        supported_commands: vec!["list_tabs".to_string(), "navigate".to_string()],
    };
    assert_eq!(
        round_trip(&register),
        json!({"type":"browser_host_register","id":1,"supportedCommands":["list_tabs","navigate"]})
    );
    let unregister = ClientMessage::BrowserHostUnregister {
        id: 2,
        host_id: "7.1".to_string(),
    };
    assert_eq!(
        round_trip(&unregister),
        json!({"type":"browser_host_unregister","id":2,"hostId":"7.1"})
    );
    let registered = DaemonMessage::BrowserHostRegistered {
        id: 1,
        host_id: "7.1".to_string(),
    };
    assert_eq!(
        round_trip(&registered),
        json!({"type":"browser_host_registered","id":1,"hostId":"7.1"})
    );
}

#[test]
fn the_execute_request_event_carries_the_caller_the_daemon_filled() {
    let request = DaemonMessage::BrowserExecuteRequest(BrowserExecuteRequest {
        request_id: "browser-1".to_string(),
        host_id: "7.1".to_string(),
        command: "navigate".to_string(),
        args: json!({"browserId":"b1"}),
        caller: BrowserCaller {
            caller_session_id: "s.a.1".to_string(),
            workspace_id: Some("w1".to_string()),
        },
    });
    assert_eq!(
        round_trip(&request),
        json!({
            "type":"browser_execute_request",
            "requestId":"browser-1",
            "hostId":"7.1",
            "command":"navigate",
            "args":{"browserId":"b1"},
            "caller":{"callerSessionId":"s.a.1","workspaceId":"w1"}
        })
    );
    let no_workspace = BrowserCaller {
        caller_session_id: "s.a.1".to_string(),
        workspace_id: None,
    };
    assert_eq!(
        round_trip(&no_workspace),
        json!({"callerSessionId":"s.a.1"}),
        "an absent workspace is an absent key, not null"
    );
}

#[test]
fn the_execute_response_carries_ok_or_a_typed_failure() {
    let ok = ClientMessage::BrowserExecuteResponse {
        id: 3,
        request_id: "browser-1".to_string(),
        host_id: "7.1".to_string(),
        outcome: BrowserOutcome::Ok {
            result: json!({"tabs":[]}),
        },
    };
    assert_eq!(
        round_trip(&ok),
        json!({
            "type":"browser_execute_response","id":3,"requestId":"browser-1","hostId":"7.1",
            "outcome":{"status":"ok","result":{"tabs":[]}}
        })
    );
    let failed = ClientMessage::BrowserExecuteResponse {
        id: 4,
        request_id: "browser-2".to_string(),
        host_id: "7.1".to_string(),
        outcome: BrowserOutcome::Err(BrowserError {
            code: BrowserErrorCode::TabNotFound,
            message: "no such tab".to_string(),
            retryable: false,
        }),
    };
    assert_eq!(
        round_trip(&failed),
        json!({
            "type":"browser_execute_response","id":4,"requestId":"browser-2","hostId":"7.1",
            "outcome":{"status":"err","code":"browser_tab_not_found","message":"no such tab","retryable":false}
        })
    );
}

#[test]
fn a_refusal_names_its_browser_code_in_the_error_details() {
    let error = WireError::new(ErrorCode::InvalidRequest, "result too large").with_details(
        ErrorDetails::BrowserRefused {
            code: BrowserErrorCode::ResultTooLarge,
        },
    );
    assert_eq!(
        round_trip(&error)["details"],
        json!({"type":"browser_refused","code":"browser_result_too_large"})
    );
}

#[test]
fn a_payload_at_the_cap_still_fits_one_frame() {
    // Compact JSON of exactly the cap: `{"s":"…"}` is 8 bytes of wrapper.
    let filler = "x".repeat(MAX_BROWSER_PAYLOAD_BYTES - 8);
    let result = json!({ "s": filler });
    assert_eq!(
        serde_json::to_vec(&result).expect("json").len(),
        MAX_BROWSER_PAYLOAD_BYTES
    );
    let frame = ClientMessage::BrowserExecuteResponse {
        id: u64::MAX,
        request_id: "r".repeat(64),
        host_id: "h".repeat(64),
        outcome: BrowserOutcome::Ok { result },
    };
    let bytes = serde_json::to_vec(&frame).expect("json").len();
    assert!(
        bytes <= MAX_FRAME_BYTES,
        "a response at the cap is {bytes} bytes against a {MAX_FRAME_BYTES} byte frame"
    );
}
