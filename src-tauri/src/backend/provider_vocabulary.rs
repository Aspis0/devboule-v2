//! The provider-vocabulary query at the app boundary — a deliberate refusal
//! for now.
//!
//! The wire shape and the three-valued `present`/`none`/`absent` behaviour
//! are specified by `reports/remote-agents/SPEC-provider-vocabulary-query.md`
//! (§4-§6). The daemon half does not exist yet: another pass builds
//! `DaemonClient::provider_vocabulary_get` and the
//! `DaemonMessage::ProviderVocabulary` reply against that same spec, and the
//! body below is replaced by the real forward then. Until that pass lands
//! this command answers every request with the refusal sentence, so the
//! absence is written down rather than silent. The frontend never sends the
//! request unless the daemon's handshake advertises the `provider_vocabulary`
//! capability, and no daemon advertises it today, so this error is
//! unreachable from the UI until the daemon side ships.

use devboule_protocol::ErrorCode;

use crate::backend::error::CommandError;

#[tauri::command]
pub fn provider_vocabulary_get(provider: String, refresh: bool) -> Result<(), CommandError> {
    Err(CommandError::new(
        ErrorCode::Internal,
        format!(
            "provider_vocabulary_get is specified but not implemented by the daemon yet \
             (provider: {provider}, refresh: {refresh}); \
             see reports/remote-agents/SPEC-provider-vocabulary-query.md"
        ),
    ))
}
