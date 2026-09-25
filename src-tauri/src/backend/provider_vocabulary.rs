//! The provider-vocabulary query at the app boundary: what one provider
//! offers, so Settings → Agents can author a profile without inventing
//! vocabulary.
//!
//! The daemon owns the answer — its cache, its probes, the three-valued
//! `present`/`none`/`absent` behaviour specified by
//! `reports/remote-agents/SPEC-provider-vocabulary-query.md` (§4-§6). This
//! command forwards one request and hands the daemon's frame on minus its
//! request id, exactly as `agent_profiles_get` hands on the document frame.
//! Nothing on this side derives vocabulary: a caller naming a provider the
//! daemon does not know receives the daemon's own refusal, verbatim.
//!
//! The frontend never sends the request unless the daemon's handshake
//! advertises the `provider_vocabulary` capability — the daemon client gates
//! on the same capability, so a daemon older than the query is refused before
//! a frame it cannot read is sent.

use std::sync::Arc;

use devboule_daemon::DaemonClient;
use devboule_protocol::{
    DaemonMessage, ErrorCode, VocabularyFeatures, VocabularyModels, VocabularyModes,
    VocabularySource,
};
use tauri::State;

use super::blocking::off_main_thread;
use super::error::CommandError;
use crate::client::DaemonBridge;

fn require_client(bridge: &DaemonBridge) -> Result<Arc<DaemonClient>, CommandError> {
    bridge
        .client()
        .map_err(|message| CommandError::new(ErrorCode::Io, message))
}

/// A reply of the wrong variant. It names no frame on purpose.
fn unexpected_reply() -> CommandError {
    CommandError::new(ErrorCode::Internal, "unexpected daemon reply")
}

/// The `provider_vocabulary_get` reply: the daemon's `ProviderVocabulary`
/// frame without its request id. The axis types are the protocol's own, so
/// the keys the panel reads are the keys the wire carries — one spelling, not
/// two.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderVocabularyReply {
    pub provider: String,
    pub models: VocabularyModels,
    pub modes: VocabularyModes,
    /// The features axis, `None` when the daemon answered without the field —
    /// which is a daemon older than the axis, and reads as "no features".
    /// Optional here for the reason it is optional on the wire: the object
    /// below is what TypeScript reads, and a key the daemon may omit must be
    /// typed optional or the panel throws on reading it.
    pub features: Option<VocabularyFeatures>,
    pub source: VocabularySource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probed_at_ms: Option<u64>,
}

#[tauri::command]
pub async fn provider_vocabulary_get(
    bridge: State<'_, DaemonBridge>,
    provider: String,
    model: Option<String>,
    refresh: bool,
) -> Result<ProviderVocabularyReply, CommandError> {
    let client = require_client(&bridge)?;
    off_main_thread(move || {
        match client.provider_vocabulary_get(&provider, model.as_deref(), refresh)? {
            DaemonMessage::ProviderVocabulary {
                provider,
                models,
                modes,
                features,
                source,
                probed_at_ms,
                ..
            } => Ok(ProviderVocabularyReply {
                provider,
                models,
                modes,
                features,
                source,
                probed_at_ms,
            }),
            _ => Err(unexpected_reply()),
        }
    })
    .await
}
