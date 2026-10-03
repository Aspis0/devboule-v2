//! What the broker's test modules share: a connection stood up as a bare
//! `ConnOut`, so what the host would receive is exactly what the broker queued,
//! and callers that run `execute` on their own thread.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::*;

pub(super) const SHORT: Duration = Duration::from_millis(250);
pub(super) const LONG: Duration = Duration::from_secs(30);

pub(super) fn caller_in(workspace_id: Option<&str>) -> BrowserCaller {
    BrowserCaller {
        caller_session_id: "s.agent.1".to_string(),
        workspace_id: workspace_id.map(str::to_string),
    }
}

pub(super) fn caller() -> BrowserCaller {
    caller_in(Some("w1"))
}

pub(super) struct FakeHost {
    pub(super) conn_id: u64,
    pub(super) out: Arc<ConnOut>,
    pub(super) host_id: String,
}

pub(super) fn host(broker: &BrowserBroker, conn_id: u64, commands: &[&str]) -> FakeHost {
    let out = ConnOut::new();
    let host_id = broker.register(
        conn_id,
        Arc::clone(&out),
        commands.iter().map(|name| (*name).to_string()).collect(),
    );
    FakeHost {
        conn_id,
        out,
        host_id,
    }
}

impl FakeHost {
    /// The next command the broker pushed to this host.
    pub(super) fn next_request(&self) -> BrowserExecuteRequest {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            for message in self.out.pull_replies() {
                if let DaemonMessage::BrowserExecuteRequest(request) = message {
                    return request;
                }
            }
            assert!(Instant::now() < deadline, "no request reached the host");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub(super) fn queued(&self) -> usize {
        self.out.pull_replies().len()
    }

    pub(super) fn answer(
        &self,
        broker: &BrowserBroker,
        request: &BrowserExecuteRequest,
        outcome: BrowserOutcome,
    ) -> ResponseDisposition {
        broker.accept_response(self.conn_id, &request.request_id, &request.host_id, outcome)
    }
}

pub(super) fn ok(result: Value) -> BrowserOutcome {
    BrowserOutcome::Ok { result }
}

pub(super) fn call_as(
    broker: &Arc<BrowserBroker>,
    caller: BrowserCaller,
    command: &'static str,
    browser_id: Option<&'static str>,
    timeout: Duration,
) -> JoinHandle<Result<Value, BrowserError>> {
    let broker = Arc::clone(broker);
    std::thread::spawn(move || {
        broker.execute(&caller, command, json!({}), browser_id, Some(timeout))
    })
}

pub(super) fn call(
    broker: &Arc<BrowserBroker>,
    command: &'static str,
    browser_id: Option<&'static str>,
    timeout: Duration,
) -> JoinHandle<Result<Value, BrowserError>> {
    call_as(broker, caller(), command, browser_id, timeout)
}

pub(super) fn wait_for_pending(broker: &BrowserBroker, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while broker.pending_len() != count {
        assert!(
            Instant::now() < deadline,
            "pending stayed at {}",
            broker.pending_len()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub(super) fn code_of(result: Result<Value, BrowserError>) -> BrowserError {
    result.expect_err("the call must fail")
}
