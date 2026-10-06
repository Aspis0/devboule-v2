//! The page's own navigation hooks, asked one question: may this tab go
//! there? The answer is the destination policy's, for the audience the tab's
//! drive earns, and a refusal is left where the agent's next answer finds it.

use std::sync::Arc;

use tauri::Url;

use super::destination::{Audience, DestinationPolicy};
use super::registry::AgentDrive;

#[derive(Clone)]
pub(super) struct TabWatch {
    policy: Arc<DestinationPolicy>,
    drive: Arc<AgentDrive>,
}

impl TabWatch {
    pub(super) fn new(policy: Arc<DestinationPolicy>, drive: Arc<AgentDrive>) -> Self {
        Self { policy, drive }
    }

    /// The refusal for one top-level navigation of this tab, if any. A tab an
    /// agent opened or acted on is the agent's even while a person is looking
    /// at it; a tab no agent has touched is never checked.
    pub(super) fn blocked(&self, candidate: &Url) -> Option<String> {
        match self.policy.admit(candidate, self.drive.audience()) {
            Ok(()) => None,
            Err(blocked) => {
                self.drive.note_refusal(blocked.message().to_string());
                Some(blocked.to_string())
            }
        }
    }

    /// The refusal for one frame navigation, from the address as the webview
    /// reports it. Only a network address is a destination: `about:`, `blob:`
    /// and `data:` frames carry their own content and reach nothing by being
    /// loaded. An address that cannot be read is refused for an agent's tab,
    /// since nothing says where it goes.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(super) fn blocked_frame(&self, raw: &str) -> Option<String> {
        match Url::parse(raw) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => self.blocked(&url),
            Ok(_) => None,
            Err(_) => (self.drive.audience() == Audience::Agent)
                .then(|| "That frame's address could not be read.".to_string()),
        }
    }
}

#[cfg(test)]
#[path = "tab_watch_tests.rs"]
mod tests;
