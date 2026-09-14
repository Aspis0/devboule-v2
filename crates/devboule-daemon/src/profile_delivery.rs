//! What a creation delivers to the child, as one typed value.
//!
//! The consent card names the profile's provider, model, mode, thinking
//! option and features. Until this struct existed, `AgentCreation` carried
//! the provider and the mode and nothing else of what the card printed: the
//! model, the thinking option and the `autoAccept` feature died at the card,
//! and the child started on the provider's default model. This module gives
//! the delivery one shape so the signature cannot grow a fourth scalar the
//! way an argument list does — the provider-trait refactor's gate (`DESIGN-
//! provider-trait-refactor.md` §8.1) names exactly this surface:
//!
//! > `session.rs` passes **one typed `ProfileDelivery`** and holds no
//! > per-family branch; a `match` on a provider id, or a per-family `if`
//! > outside a client module, is a defect, not an implementation.
//!
//! Two of the four fields are **constraints on the child's start**, not
//! configuration to hand over:
//!
//! - `auto_accept` is a constraint on which mode is delivered: the child
//!   must start in a mode that answers its own permission prompts. It is
//!   validated by the client that owns the mode knob, and a delivered mode
//!   that does not answer is a refusal, never a substitution.
//! - `mode_id` is `None` only for a create that resolved no profile (the
//!   human's provider picker, a terminal); a child from a profile always
//!   carries one, because the store refuses a profile without a mode.
//!
//! Application lives in each client module (argv, state seed, or
//! post-handshake wire); this module holds no per-family knowledge at all.

use serde_json::Value;

/// One delivery: everything the child is started with because a profile
/// named it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProfileDelivery {
    /// The mode the child is switched into. `None` means "the provider's
    /// own default", which is what a create that resolved no profile asks
    /// for; a profile's child always names one.
    pub(crate) mode_id: Option<String>,
    /// The model the child must run, exactly as the profile saved it.
    /// `None` for a create that resolved no profile.
    pub(crate) model_id: Option<String>,
    /// The thinking option the child must start with. `None` delivers
    /// nothing: a profile with no thinking option promises none.
    pub(crate) thinking_option_id: Option<String>,
    /// The profile's `autoAccept` tick, carried as the constraint it is:
    /// the delivered mode must answer the child's own permission prompts,
    /// and the client that owns the mode refuses a child that would ask.
    pub(crate) auto_accept: bool,
}

impl ProfileDelivery {
    /// What a create that resolved no profile delivers: nothing. The
    /// provider starts on its own defaults, exactly as before this module.
    pub(crate) fn none() -> Self {
        Self {
            mode_id: None,
            model_id: None,
            thinking_option_id: None,
            auto_accept: false,
        }
    }

    /// What a human's create delivers: the requested mode, if one was
    /// named, and nothing else.
    pub(crate) fn for_request(mode_id: Option<String>) -> Self {
        Self {
            mode_id,
            ..Self::none()
        }
    }

    /// What a child created from a resolved profile delivers: the profile's
    /// own values, verbatim. The model and the mode arrive required (the
    /// store refuses a profile without them); the thinking option arrives
    /// optional, and `None` delivers nothing.
    pub(crate) fn for_child(
        mode_id: &str,
        model_id: &str,
        thinking_option_id: Option<&str>,
        features: &serde_json::Map<String, Value>,
    ) -> Self {
        Self {
            mode_id: Some(mode_id.to_string()),
            model_id: Some(model_id.to_string()),
            thinking_option_id: thinking_option_id.map(str::to_string),
            auto_accept: feature_is_true(features, crate::provider_catalog::AUTO_ACCEPT_FEATURE),
        }
    }
}

/// The one reading of a feature tick: the JSON boolean `true`, and nothing
/// else. A `"true"` string or a number is a value somebody stored, not a
/// tick, and no derivation may read one as a tick.
pub(crate) fn feature_is_true(features: &serde_json::Map<String, Value>, key: &str) -> bool {
    matches!(features.get(key), Some(Value::Bool(true)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn features(value: serde_json::Value) -> serde_json::Map<String, Value> {
        value.as_object().expect("object").clone()
    }

    /// A create that resolved no profile delivers nothing at all.
    #[test]
    fn none_delivers_nothing() {
        let delivery = ProfileDelivery::none();
        assert_eq!(delivery.mode_id, None);
        assert_eq!(delivery.model_id, None);
        assert_eq!(delivery.thinking_option_id, None);
        assert!(!delivery.auto_accept);
    }

    /// A human's create carries the requested mode and nothing else.
    #[test]
    fn for_request_carries_only_the_mode() {
        let delivery = ProfileDelivery::for_request(Some("bypass".to_string()));
        assert_eq!(delivery.mode_id.as_deref(), Some("bypass"));
        assert_eq!(delivery.model_id, None);
        assert_eq!(delivery.thinking_option_id, None);
        assert!(!delivery.auto_accept);
        assert_eq!(ProfileDelivery::for_request(None), ProfileDelivery::none());
    }

    /// A profile's child carries the profile's own values, verbatim.
    #[test]
    fn for_child_carries_the_profile_verbatim() {
        let delivery = ProfileDelivery::for_child(
            "bypassPermissions",
            "claude-opus-5",
            Some("high"),
            &features(json!({"autoAccept": true})),
        );
        assert_eq!(delivery.mode_id.as_deref(), Some("bypassPermissions"));
        assert_eq!(delivery.model_id.as_deref(), Some("claude-opus-5"));
        assert_eq!(delivery.thinking_option_id.as_deref(), Some("high"));
        assert!(delivery.auto_accept);
    }

    /// No thinking option delivers nothing — a profile that promises none
    /// delivers none, which is fine everywhere.
    #[test]
    fn a_profile_without_a_thinking_option_delivers_none() {
        let delivery = ProfileDelivery::for_child("ask", "stub-model", None, &features(json!({})));
        assert_eq!(delivery.thinking_option_id, None);
        assert!(!delivery.auto_accept);
    }

    /// The tick is the boolean `true` and only that: the same reading
    /// `profile_is_unattended` makes, so the card, the marker and the
    /// delivery cannot disagree about what a tick is.
    #[test]
    fn the_tick_is_only_the_boolean_true() {
        assert!(feature_is_true(
            &features(json!({"autoAccept": true})),
            crate::provider_catalog::AUTO_ACCEPT_FEATURE
        ));
        assert!(!feature_is_true(
            &features(json!({"autoAccept": "true"})),
            crate::provider_catalog::AUTO_ACCEPT_FEATURE,
        ));
        assert!(!feature_is_true(
            &features(json!({"autoAccept": false})),
            crate::provider_catalog::AUTO_ACCEPT_FEATURE
        ));
        assert!(!feature_is_true(
            &features(json!({})),
            crate::provider_catalog::AUTO_ACCEPT_FEATURE
        ));
    }
}
