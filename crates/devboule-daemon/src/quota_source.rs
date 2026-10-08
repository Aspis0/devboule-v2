//! The seam every account quota source plugs into: a source names the provider
//! id its frame is stored under, the one destination it may reach, the request
//! for a key, and how a response becomes windows. [`fetch`] runs that exchange
//! through the guarded client and returns the frame, or the reason there is
//! none.

use std::time::Duration;

use devboule_protocol::{PlanWindow, SessionEvent};

use crate::egress_client::{send, Answer, Limits, OutboundError, Request};
use crate::egress_policy::Rule;
use crate::quota_key::ApiKey;

/// For the whole exchange, resolution included.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// A usage response is a few hundred bytes; a body past this is not one.
const MAX_BODY: usize = 64 * 1024;

pub(crate) trait QuotaSource {
    /// The provider id the frame is stored and shown under.
    fn provider_id(&self) -> &'static str;
    /// The plan's name, as the app prints it.
    fn plan_label(&self) -> &'static str;
    /// The one destination this source may reach.
    fn rule(&self) -> &'static Rule;
    /// The request for a key. The key appears only in its Authorization header.
    fn request(&self, key: &ApiKey) -> Request;
    /// The windows a 200 body carries. A body that cannot be read is an error,
    /// never a zero reading.
    fn windows(&self, body: &[u8], observed_at_ms: i64) -> Result<Vec<PlanWindow>, QuotaError>;
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum QuotaError {
    /// The provider refused the key (401 or 403).
    Rejected,
    /// The provider asked for less traffic (429), with its own delay in seconds
    /// when it named one.
    Throttled { retry_after_secs: Option<u64> },
    /// Any other status the source does not read as a reading.
    Status(u16),
    /// The answer held no usable reading.
    Malformed,
    /// The exchange was refused or failed on the way: a timeout, a transport
    /// failure, a cut or oversized body, or a destination the policy refused.
    Outbound(OutboundError),
}

/// One exchange with the given sender, and the frame its answer makes. The
/// sender is the guarded client in production and a scripted one in tests.
pub(crate) fn fetch_with<S>(
    source: &dyn QuotaSource,
    key: &ApiKey,
    observed_at_ms: i64,
    sender: S,
) -> Result<SessionEvent, QuotaError>
where
    S: FnOnce(&Rule, &Request, &Limits) -> Result<Answer, OutboundError>,
{
    let request = source.request(key);
    let limits = Limits {
        timeout: FETCH_TIMEOUT,
        max_body: MAX_BODY,
        max_redirects: 0,
    };
    let answer = sender(source.rule(), &request, &limits).map_err(QuotaError::Outbound)?;
    match answer.status {
        200 => {}
        401 | 403 => return Err(QuotaError::Rejected),
        429 => {
            return Err(QuotaError::Throttled {
                retry_after_secs: answer.retry_after_secs,
            })
        }
        status => return Err(QuotaError::Status(status)),
    }
    let windows = source.windows(&answer.body, observed_at_ms)?;
    if windows.is_empty() {
        return Err(QuotaError::Malformed);
    }
    Ok(SessionEvent::PlanUsage {
        provider_id: source.provider_id().to_string(),
        plan_label: Some(source.plan_label().to_string()),
        windows,
        credits: None,
        observed_at_ms: Some(observed_at_ms),
    })
}

/// One production exchange, through the guarded client.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn fetch(
    source: &dyn QuotaSource,
    key: &ApiKey,
    observed_at_ms: i64,
) -> Result<SessionEvent, QuotaError> {
    fetch_with(source, key, observed_at_ms, |rule, request, limits| {
        send(rule, request, limits)
    })
}

#[cfg(test)]
#[path = "quota_source_tests.rs"]
mod tests;
