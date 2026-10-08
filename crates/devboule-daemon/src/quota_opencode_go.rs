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

/// Above this an epoch number is milliseconds, not seconds.
const MILLISECONDS_ABOVE: i64 = 100_000_000_000;

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

/// A reset moment in Unix seconds: a number (seconds, or milliseconds past
/// [`MILLISECONDS_ABOVE`]), a digit string of either, or an RFC 3339 timestamp.
fn reset_seconds(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| {
                number
                    .as_f64()
                    .filter(|value| value.is_finite())
                    .map(|value| value.floor() as i64)
            })
            .map(epoch_seconds),
        Value::String(text) => {
            let text = text.trim();
            text.parse::<i64>()
                .ok()
                .map(epoch_seconds)
                .or_else(|| rfc3339_seconds(text))
        }
        _ => None,
    }
}

fn epoch_seconds(value: i64) -> i64 {
    if value.abs() > MILLISECONDS_ABOVE {
        value.div_euclid(1000)
    } else {
        value
    }
}

/// An RFC 3339 timestamp (`2026-10-07T18:00:00Z`, fractions and `±hh:mm`
/// offsets allowed) as Unix seconds. Anything else is none.
fn rfc3339_seconds(text: &str) -> Option<i64> {
    let (date, rest) = text.split_once(['T', 't', ' '])?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() {
        return None;
    }
    let zone_at = rest.find(['Z', 'z', '+', '-'])?;
    let (clock, zone) = rest.split_at(zone_at);
    let clock = clock.split('.').next()?;
    let mut clock_parts = clock.split(':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let second: i64 = clock_parts.next()?.parse().ok()?;
    if clock_parts.next().is_some() {
        return None;
    }
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let offset_minutes = match zone {
        "Z" | "z" => 0,
        _ => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (zone_hours, zone_minutes) = zone[1..].split_once(':')?;
            let zone_hours: i64 = zone_hours.parse().ok()?;
            let zone_minutes: i64 = zone_minutes.parse().ok()?;
            sign * (zone_hours * 60 + zone_minutes)
        }
    };
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60)
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
#[path = "quota_opencode_go_tests.rs"]
mod tests;
