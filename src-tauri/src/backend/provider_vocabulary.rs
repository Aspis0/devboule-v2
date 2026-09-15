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
    DaemonMessage, ErrorCode, VocabularyModels, VocabularyModes, VocabularySource,
};
use tauri::State;

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
    pub source: VocabularySource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probed_at_ms: Option<u64>,
}

#[tauri::command]
pub fn provider_vocabulary_get(
    bridge: State<'_, DaemonBridge>,
    provider: String,
    refresh: bool,
) -> Result<ProviderVocabularyReply, CommandError> {
    match require_client(&bridge)?.provider_vocabulary_get(&provider, refresh)? {
        DaemonMessage::ProviderVocabulary {
            provider,
            models,
            modes,
            source,
            probed_at_ms,
            ..
        } => Ok(ProviderVocabularyReply {
            provider,
            models,
            modes,
            source,
            probed_at_ms,
        }),
        _ => Err(unexpected_reply()),
    }
}
