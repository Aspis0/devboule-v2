//! OpenCode Go's account usage, from OpenCode's own usage route. The route
//! computes the windows on its side: `rolling` is the 5-hour window, `weekly`
//! the week. The monthly entry is not shown, since the plan's popover has no
//! monthly row.

use devboule_protocol::PlanWindow;
use serde_json::Value;

use crate::egress_client::{Method, Request};
use crate::egress_policy::Rule;
use crate::quota_key::ApiKey;
use crate::quota_source::{QuotaError, QuotaSource};

const HOST: &str = "opencode.ai";
const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";
const RULE: Rule = Rule {
    hosts: &[HOST],
    loopback_path: None,
};

/// The route's entry keys, each with its window in minutes.
const WINDOWS: [(&str, u64); 2] = [("rolling", 300), ("weekly", 10_080)];

/// The plausible band for a reset in Unix seconds: 2001-09-09 up to 5138-11-16.
/// A number outside it is shown as no reset, never reinterpreted: the route is
/// documented in seconds, so there is no millisecond reading to fall back on.
const EPOCH_SECONDS_FROM: i64 = 1_000_000_000;
const EPOCH_SECONDS_BELOW: i64 = 100_000_000_000;

/// The OpenCode Go plan, read from the account usage route.
pub(crate) struct OpencodeGo;

impl QuotaSource for OpencodeGo {
    fn provider_id(&self) -> &'static str {
        "opencode-go"
    }

    fn plan_label(&self) -> &'static str {
        "OpenCode Go"
    }

    fn rule(&self) -> &'static Rule {
        &RULE
    }

    fn request(&self, key: &ApiKey) -> Request {
        Request {
            method: Method::Get,
            url: USAGE_URL.to_string(),
            headers: vec![
                ("authorization", key.bearer()),
                ("accept", "application/json".to_string()),
            ],
            body: Vec::new(),
        }
    }

    fn windows(&self, body: &[u8], _observed_at_ms: i64) -> Result<Vec<PlanWindow>, QuotaError> {
        windows_from_body(body)
    }
}

/// The windows a usage body names, in the popover's order. An absent window is
/// left out, never shown as zero.
fn windows_from_body(body: &[u8]) -> Result<Vec<PlanWindow>, QuotaError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| QuotaError::Malformed)?;
    let usage = value
        .get("usage")
        .and_then(Value::as_object)
        .ok_or(QuotaError::Malformed)?;
    Ok(WINDOWS
        .iter()
        .filter_map(|(key, duration_mins)| {
            usage
                .get(*key)
                .and_then(|entry| window(entry, *duration_mins))
        })
        .collect())
}

/// One entry as a window: its percent when a usable one came, its reset when
/// one came. An entry with neither names no window.
fn window(entry: &Value, duration_mins: u64) -> Option<PlanWindow> {
    let object = entry.as_object()?;
    let used_percent = object
        .get("percent")
        .and_then(Value::as_f64)
        .filter(|percent| percent.is_finite() && *percent >= 0.0)
        .map(|percent| percent.round() as u64);
    let resets_at = object.get("resetsAt").and_then(reset_seconds);
    (used_percent.is_some() || resets_at.is_some()).then_some(PlanWindow {
        duration_mins,
        used_percent,
        resets_at,
    })
}

/// A reset moment in Unix seconds: a number or a digit string of seconds inside
/// the plausible band, or an RFC 3339 timestamp. Anything else is no reset.
fn reset_seconds(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => match number.as_i64() {
            Some(seconds) => plausible_seconds(seconds),
            None => number
                .as_f64()
                .filter(|seconds| {
                    seconds.is_finite()
                        && (EPOCH_SECONDS_FROM as f64..EPOCH_SECONDS_BELOW as f64).contains(seconds)
                })
                .map(|seconds| seconds.floor() as i64),
        },
        Value::String(text) => {
            let text = text.trim();
            match text.parse::<i64>() {
                Ok(seconds) => plausible_seconds(seconds),
                Err(_) => rfc3339_seconds(text),
            }
        }
        _ => None,
    }
}

fn plausible_seconds(seconds: i64) -> Option<i64> {
    (EPOCH_SECONDS_FROM..EPOCH_SECONDS_BELOW)
        .contains(&seconds)
        .then_some(seconds)
}

/// An RFC 3339 timestamp (`2026-10-07T18:00:00Z`, fractions and `±hh:mm`
/// offsets) as Unix seconds. The library refuses impossible dates (Feb 30,
/// month 13) and offsets without a colon (`+0000`); those are no reset.
fn rfc3339_seconds(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .and_then(|moment| plausible_seconds(moment.timestamp()))
}

#[cfg(test)]
#[path = "quota_opencode_go_tests.rs"]
mod tests;
