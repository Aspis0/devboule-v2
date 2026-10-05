//! An in-process DevTools endpoint: a loopback websocket that answers CDP
//! commands from a script, so the transport's routing is proved against a real
//! socket rather than a reader that agrees with it.
//!
//! The script is consumed in the order commands arrive, one step per command,
//! which is what lets a test say what the first call gets and what the second
//! one gets.

use std::collections::VecDeque;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use super::{WsEvents, WsPage, MAX_MESSAGE};

/// What the endpoint does with the next command it reads.
#[derive(Clone)]
pub enum Step {
    /// Answer with the command's own id and method, which is what tells a test
    /// which waiter a reply reached.
    Echo,
    /// Answer with a result of the test's choosing instead of the echo, for a
    /// command whose payload the test needs.
    Answer(Value),
    /// Answer with the protocol's own error object.
    Error { code: i64, message: String },
    /// Answer with a fixed result object, for a command whose answer a test
    /// has to shape and the echo cannot.
    Answer { result: Value },
    /// Say an event, then answer the command with its own id and method.
    Event { method: String, params: Value },
    /// Answer a call that names a node the way a page's own document does: a
    /// box while the document the ref was taken in is current, and the
    /// runtime's own refusal once a `Page.frameNavigated` has been sent. What
    /// makes a stale-ref test prove the event did it, rather than a script.
    Ref,
    /// Say a frame that is not a protocol message at all, then answer.
    NotAMessage,
    /// Answer with one message past the transport's cap, so the reader has to
    /// survive a peer that breaks the limit rather than a peer that stops.
    Oversize,
    /// Keep this command's answer back until a release sends it out.
    Hold,
    /// Answer this command first, then every held answer, newest first.
    Release,
    /// Answer every held answer before this command's own, so a reply for a
    /// waiter that is already gone lands while another call is still waiting.
    ReleaseHeldFirst,
    /// Say nothing at all, so the call runs out of its budget.
    Silence,
    /// Drop the socket under the call that is waiting on it.
    Disconnect,
}

/// One loopback endpoint serving one connection.
pub struct FakeServer {
    url: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl FakeServer {
    /// Listen on a loopback port and start serving `script` to the first page
    /// that connects. The port is never reused, so two endpoints in one test
    /// run never collide.
    pub async fn start(script: Vec<Step>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let port = listener.local_addr().expect("the port bound").port();
        let steps = Arc::new(Mutex::new(VecDeque::from(script)));
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let written = Arc::clone(&seen);
        tauri::async_runtime::spawn(async move {
            let (stream, _) = listener.accept().await.expect("the page connects");
            let Ok(mut socket) = accept_async(stream).await else {
                return;
            };
            let mut held: Vec<Value> = Vec::new();
            // Whether this target has committed a document, which is what a
            // ref taken before the last one no longer has.
            let mut committed = false;
            while let Some(Ok(frame)) = socket.next().await {
                let Message::Text(text) = frame else { continue };
                let Ok(command) = serde_json::from_str::<Value>(text.as_str()) else {
                    continue;
                };
                let id = command["id"].clone();
                written
                    .lock()
                    .await
                    .push(command["method"].as_str().unwrap_or_default().to_owned());
                let echoed = json!({ "id": id, "result": { "method": command["method"] } });
                let step = steps.lock().await.pop_front().unwrap_or(Step::Silence);
                let answers = match step {
                    Step::Echo => vec![echoed],
                    Step::Answer(result) => vec![json!({ "id": id, "result": result })],
                    Step::Ref => vec![if committed {
                        json!({ "id": id, "error": { "code": -32000, "message": "No node with given id found" } })
                    } else {
                        json!({
                            "id": id,
                            "result": { "model": { "content": [0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0] } }
                        })
                    }],
                    Step::Error { code, message } => {
                        vec![json!({ "id": id, "error": { "code": code, "message": message } })]
                    }
                    Step::Answer { result } => vec![json!({ "id": id, "result": result })],
                    Step::Event { method, params } => {
                        if method == "Page.frameNavigated" {
                            committed = true;
                        }
                        let event = json!({ "method": method, "params": params });
                        vec![event, echoed]
                    }
                    Step::NotAMessage => {
                        vec![json!({ "detail": "not a protocol message" }), echoed]
                    }
                    Step::Oversize => {
                        let filler = "x".repeat(MAX_MESSAGE + 1024);
                        vec![json!({ "id": id, "result": { "data": filler } })]
                    }
                    Step::Hold => {
                        held.push(echoed);
                        continue;
                    }
                    Step::Release => {
                        held.push(echoed);
                        held.reverse();
                        std::mem::take(&mut held)
                    }
                    Step::ReleaseHeldFirst => {
                        held.push(echoed);
                        std::mem::take(&mut held)
                    }
                    Step::Silence => continue,
                    Step::Disconnect => break,
                };
                for answer in answers {
                    if socket
                        .send(Message::text(answer.to_string()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
        });
        FakeServer {
            url: format!("ws://127.0.0.1:{port}/devtools/page/1"),
            seen,
        }
    }

    /// The address a page target would hand out for its debugger socket.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The methods whose frames this endpoint has actually read, which is how
    /// a test tells "the call was refused" from "the call was never sent".
    pub async fn read(&self) -> Vec<String> {
        self.seen.lock().await.clone()
    }

    /// The endpoint and a page already attached to it, with the event streams
    /// that page's reader hands back.
    pub async fn attached(self) -> (Self, WsPage, WsEvents) {
        let (page, events) = WsPage::connect(self.url())
            .await
            .expect("the loopback endpoint is opened");
        (self, page, events)
    }
}
