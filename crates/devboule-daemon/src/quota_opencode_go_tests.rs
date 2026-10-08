//! OpenCode Go's usage body and request, from fixture JSON. No test reaches
//! opencode.ai: the exchange is covered with a scripted transport elsewhere.

use devboule_protocol::PlanWindow;
use serde_json::json;

use super::{windows_from_body, OpencodeGo, HOST};
use crate::egress_client::Method;
use crate::quota_key::ApiKey;
use crate::quota_source::{QuotaError, QuotaSource};

/// 2026-10-07T18:00:00Z in Unix seconds.
const OCT_7_18H: i64 = 1_791_396_000;

fn body(value: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&value).expect("fixture json")
}

fn parse(value: serde_json::Value) -> Result<Vec<PlanWindow>, QuotaError> {
    windows_from_body(&body(value))
}

#[test]
fn both_windows_come_out_in_popover_order_with_their_durations() {
    let windows = parse(json!({
        "usage": {
            "weekly": { "status": "ok", "percent": 41, "resetsAt": OCT_7_18H },
            "rolling": { "status": "ok", "percent": 58, "resetsAt": OCT_7_18H - 3600 },
            "monthly": { "status": "ok", "percent": 12, "resetsAt": OCT_7_18H }
        }
    }))
    .expect("windows");
    assert_eq!(
        windows,
        vec![
            PlanWindow {
                duration_mins: 300,
                used_percent: Some(58),
                resets_at: Some(OCT_7_18H - 3600),
            },
            PlanWindow {
                duration_mins: 10_080,
                used_percent: Some(41),
                resets_at: Some(OCT_7_18H),
            },
        ]
    );
}

#[test]
fn fractional_percents_round_and_overages_keep_their_size() {
    let windows = parse(json!({
        "usage": {
            "rolling": { "percent": 12.4, "resetsAt": OCT_7_18H },
            "weekly": { "percent": 137.6, "resetsAt": OCT_7_18H }
        }
    }))
    .expect("windows");
    assert_eq!(windows[0].used_percent, Some(12));
    assert_eq!(windows[1].used_percent, Some(138));
}

#[test]
fn a_negative_or_missing_percent_is_no_percent_never_zero() {
    let windows = parse(json!({
        "usage": {
            "rolling": { "percent": -3, "resetsAt": OCT_7_18H },
            "weekly": { "resetsAt": OCT_7_18H }
        }
    }))
    .expect("windows");
    assert_eq!(windows[0].used_percent, None);
    assert_eq!(windows[1].used_percent, None);
    assert_eq!(windows[0].resets_at, Some(OCT_7_18H));
}

#[test]
fn a_window_with_neither_percent_nor_reset_is_left_out() {
    let windows = parse(json!({
        "usage": {
            "rolling": { "status": "ok" },
            "weekly": { "percent": 7 }
        }
    }))
    .expect("windows");
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].duration_mins, 10_080);
    assert_eq!(windows[0].used_percent, Some(7));
    assert_eq!(windows[0].resets_at, None);
}

#[test]
fn reset_times_read_as_seconds_digit_strings_and_rfc3339() {
    let windows = parse(json!({
        "usage": {
            "rolling": { "percent": 1, "resetsAt": OCT_7_18H },
            "weekly": { "percent": 2, "resetsAt": "2026-10-07T18:00:00Z" }
        }
    }))
    .expect("windows");
    assert_eq!(windows[0].resets_at, Some(OCT_7_18H));
    assert_eq!(windows[1].resets_at, Some(OCT_7_18H));

    let text = parse(json!({
        "usage": {
            "rolling": { "percent": 1, "resetsAt": OCT_7_18H.to_string() },
            "weekly": { "percent": 2, "resetsAt": "2026-10-07T20:00:00+02:00" }
        }
    }))
    .expect("windows");
    assert_eq!(text[0].resets_at, Some(OCT_7_18H));
    assert_eq!(text[1].resets_at, Some(OCT_7_18H));
}

#[test]
fn a_millisecond_number_is_no_reset_rather_than_a_guess() {
    let windows = parse(json!({
        "usage": {
            "rolling": { "percent": 1, "resetsAt": OCT_7_18H * 1000 },
            "weekly": { "percent": 2, "resetsAt": -OCT_7_18H }
        }
    }))
    .expect("windows");
    assert_eq!(windows[0].resets_at, None);
    assert_eq!(windows[0].used_percent, Some(1));
    assert_eq!(windows[1].resets_at, None);
}

#[test]
fn numbers_outside_the_plausible_seconds_band_are_no_reset() {
    for resets_at in [
        json!(u64::MAX),
        json!(i64::MAX),
        json!(1e30),
        json!(-1e12),
        json!(1790000000.75),
    ] {
        let windows = parse(json!({
            "usage": { "rolling": { "percent": 1, "resetsAt": resets_at } }
        }))
        .expect("windows");
        let expected = (resets_at == json!(1790000000.75)).then_some(1_790_000_000);
        assert_eq!(windows[0].resets_at, expected, "{resets_at}");
    }
}

#[test]
fn impossible_calendar_dates_are_refused_and_leap_days_accepted() {
    for text in [
        "2026-02-30T00:00:00Z",
        "2026-04-31T00:00:00Z",
        "2026-02-29T00:00:00Z",
        "2026-13-01T00:00:00Z",
        "999999999999-12-31T23:59:59Z",
        "2026-10-07T20:00:00+0000",
    ] {
        let windows = parse(json!({
            "usage": { "rolling": { "percent": 1, "resetsAt": text } }
        }))
        .expect("windows");
        assert_eq!(windows[0].resets_at, None, "{text}");
    }
    let leap = parse(json!({
        "usage": { "rolling": { "percent": 1, "resetsAt": "2024-02-29T00:00:00Z" } }
    }))
    .expect("windows");
    assert_eq!(leap[0].resets_at, Some(1_709_164_800));
}

#[test]
fn a_timestamp_with_fractions_and_a_bad_one_are_read_accordingly() {
    let windows = parse(json!({
        "usage": {
            "rolling": { "percent": 1, "resetsAt": "2026-10-07T18:00:00.250Z" },
            "weekly": { "percent": 2, "resetsAt": "2026-13-40T99:00:00Z" }
        }
    }))
    .expect("windows");
    assert_eq!(windows[0].resets_at, Some(OCT_7_18H));
    assert_eq!(windows[1].resets_at, None);
    assert_eq!(windows[1].used_percent, Some(2));
}

#[test]
fn a_body_that_is_not_json_or_has_no_usage_is_malformed() {
    assert_eq!(
        windows_from_body(b"<html>denied</html>"),
        Err(QuotaError::Malformed)
    );
    assert_eq!(parse(json!({ "data": {} })), Err(QuotaError::Malformed));
    assert_eq!(parse(json!({ "usage": [] })), Err(QuotaError::Malformed));
}

#[test]
fn a_usage_object_with_no_known_window_has_no_windows() {
    assert_eq!(
        parse(json!({ "usage": { "monthly": { "percent": 9 } } })),
        Ok(Vec::new())
    );
}

#[test]
fn the_request_is_a_get_to_the_usage_route_with_the_key_only_in_its_header() {
    let key = ApiKey::from_text("fixture-key").expect("a key");
    let request = OpencodeGo.request(&key);
    assert_eq!(request.method, Method::Get);
    assert_eq!(request.url, "https://opencode.ai/zen/go/v1/usage");
    assert!(request.body.is_empty());
    let header = request
        .headers
        .iter()
        .find(|(name, _)| *name == "authorization")
        .map(|(_, value)| value.as_str());
    assert_eq!(header, Some("Bearer fixture-key"));
    assert!(!request.url.contains("fixture-key"));
}

#[test]
fn the_one_declared_host_is_opencode_and_the_identity_is_the_go_plan() {
    assert_eq!(OpencodeGo.rule().hosts, [HOST].as_slice());
    assert_eq!(OpencodeGo.rule().loopback_path, None);
    assert_eq!(OpencodeGo.provider_id(), "opencode-go");
    assert_eq!(OpencodeGo.plan_label(), "OpenCode Go");
}

