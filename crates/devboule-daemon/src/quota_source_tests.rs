//! One quota exchange through the guarded client over a scripted transport: the
//! status and body rules, the cap, the redirect refusal, and that the key shows
//! up nowhere but the Authorization header. No socket is opened.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use devboule_protocol::SessionEvent;

use crate::egress_client::{send_with, Hop, Limits, OutboundError, Raw, Transport};
use crate::egress_policy::tests::FakeResolver;
use crate::egress_policy::{Resolver, Rule};
use crate::quota_key::ApiKey;
use crate::quota_opencode_go::OpencodeGo;
use crate::quota_source::{fetch_with, QuotaError};

const KEY: &str = "fixture-key-0123456789";
const OBSERVED: i64 = 1_791_396_000_000;

/// Answers each hop from a script and records what it was asked to do.
struct Scripted {
    answers: Mutex<VecDeque<Result<Raw, OutboundError>>>,
    seen: Mutex<Vec<(String, Option<Vec<String>>)>>,
}

impl Scripted {
    fn new(answers: Vec<Result<Raw, OutboundError>>) -> Self {
        Self {
            answers: Mutex::new(answers.into()),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl Transport for Scripted {
    fn execute(&self, hop: &Hop<'_>) -> Result<Raw, OutboundError> {
        self.seen.lock().expect("seen").push((
            hop.url.to_string(),
            hop.pin
                .map(|pin| pin.addresses.iter().map(ToString::to_string).collect()),
        ));
        self.answers
            .lock()
            .expect("answers")
            .pop_front()
            .expect("the script has an answer for this hop")
    }
}

fn raw(status: u16, body: Vec<u8>) -> Result<Raw, OutboundError> {
    Ok(Raw {
        status,
        location: None,
        retry_after: None,
        body,
    })
}

fn usage_body() -> Vec<u8> {
    br#"{"usage":{"rolling":{"percent":58,"resetsAt":1791396000},"weekly":{"percent":41,"resetsAt":1791396000}}}"#
        .to_vec()
}

/// What the transport saw of each hop: the URL and the addresses it was pinned to.
type Hops = Vec<(String, Option<Vec<String>>)>;

/// Run one fetch with the scripted transport and the fixture resolver, and
/// return what the transport saw with the outcome.
fn run(answers: Vec<Result<Raw, OutboundError>>) -> (Result<SessionEvent, QuotaError>, Hops) {
    let transport = Scripted::new(answers);
    let resolver: Arc<dyn Resolver> = FakeResolver::new(&[("opencode.ai", &["104.18.0.1"])]);
    let key = ApiKey::from_text(KEY).expect("a key");
    let outcome = fetch_with(
        &OpencodeGo,
        &key,
        OBSERVED,
        |rule: &Rule, request, limits: &Limits| {
            send_with(&transport, &resolver, rule, request, limits)
        },
    );
    let seen = transport.seen.lock().expect("seen").clone();
    (outcome, seen)
}

#[test]
fn a_good_answer_becomes_the_go_frame_stamped_with_its_observation() {
    let (outcome, seen) = run(vec![raw(200, usage_body())]);
    let SessionEvent::PlanUsage {
        provider_id,
        plan_label,
        windows,
        observed_at_ms,
        ..
    } = outcome.expect("a frame")
    else {
        panic!("a plan usage frame");
    };
    assert_eq!(provider_id, "opencode-go");
    assert_eq!(plan_label.as_deref(), Some("OpenCode Go"));
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0].used_percent, Some(58));
    assert_eq!(observed_at_ms, Some(OBSERVED));
    assert_eq!(seen.len(), 1, "one hop, no redirect followed");
    assert_eq!(seen[0].0, "https://opencode.ai/zen/go/v1/usage");
    assert_eq!(seen[0].1, Some(vec!["104.18.0.1:443".to_string()]));
}

#[test]
fn a_refused_key_is_rejected_and_yields_no_frame() {
    assert_eq!(run(vec![raw(403, Vec::new())]).0, Err(QuotaError::Rejected));
    assert_eq!(run(vec![raw(401, Vec::new())]).0, Err(QuotaError::Rejected));
}

#[test]
fn any_other_status_is_a_status_error_never_a_reading() {
    assert_eq!(
        run(vec![raw(500, Vec::new())]).0,
        Err(QuotaError::Status(500))
    );
}

#[test]
fn a_body_with_no_usage_is_malformed() {
    assert_eq!(
        run(vec![raw(200, b"{\"usage\":{}}".to_vec())]).0,
        Err(QuotaError::Malformed)
    );
    assert_eq!(
        run(vec![raw(200, b"not json".to_vec())]).0,
        Err(QuotaError::Malformed)
    );
}

#[test]
fn a_timeout_is_an_outbound_timeout_and_yields_no_frame() {
    assert_eq!(
        run(vec![Err(OutboundError::Timeout)]).0,
        Err(QuotaError::Outbound(OutboundError::Timeout))
    );
}

#[test]
fn a_body_past_the_cap_is_too_large_and_yields_no_frame() {
    let oversized = vec![b' '; 64 * 1024 + 1];
    assert_eq!(
        run(vec![raw(200, oversized)]).0,
        Err(QuotaError::Outbound(OutboundError::TooLarge))
    );
}

#[test]
fn a_redirect_is_refused_rather_than_followed() {
    let redirect = Ok(Raw {
        status: 302,
        location: Some("https://elsewhere.example/usage".to_string()),
        retry_after: None,
        body: Vec::new(),
    });
    let (outcome, seen) = run(vec![redirect]);
    assert!(matches!(
        outcome,
        Err(QuotaError::Outbound(OutboundError::Refused(_)))
    ));
    assert_eq!(seen.len(), 1, "the redirect target was never requested");
}

#[test]
fn the_key_never_appears_in_the_frame_or_in_an_error() {
    let (good, _) = run(vec![raw(200, usage_body())]);
    let frame = serde_json::to_string(&good.expect("a frame")).expect("json");
    assert!(!frame.contains(KEY));
    let (rejected, _) = run(vec![raw(403, Vec::new())]);
    assert!(!format!("{rejected:?}").contains(KEY));
}

#[test]
fn a_throttled_answer_carries_the_delay_the_provider_named() {
    let named = Ok(Raw {
        status: 429,
        location: None,
        retry_after: Some("45".to_string()),
        body: Vec::new(),
    });
    let (outcome, _) = run(vec![named]);
    assert_eq!(
        outcome,
        Err(QuotaError::Throttled {
            retry_after_secs: Some(45)
        })
    );

    let dated = Ok(Raw {
        status: 429,
        location: None,
        retry_after: Some("Wed, 21 Oct 2026 07:28:00 GMT".to_string()),
        body: Vec::new(),
    });
    let (outcome, _) = run(vec![dated]);
    assert_eq!(
        outcome,
        Err(QuotaError::Throttled {
            retry_after_secs: None
        })
    );
}
