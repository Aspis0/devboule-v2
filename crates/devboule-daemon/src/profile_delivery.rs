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

use devboule_protocol::{ErrorCode, WireError};

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
    /// Every feature value the profile stores **beyond** the tick, keyed as
    /// the provider declared it and in the profile's own key order. This is
    /// the slot that makes a drawn control reach the child: each client
    /// applies the values it was given a frame for and refuses the creation
    /// over a value it has no frame for, so the rule "a child that exists was
    /// delivered everything its card printed" survives a new feature without
    /// this struct growing a field per feature.
    ///
    /// `auto_accept` is not in here, and that is not an omission: it is a
    /// constraint on which mode is delivered, not configuration to hand over
    /// (the field above), and reading it twice would let the card and the
    /// delivery disagree about what one key means.
    pub(crate) features: Vec<DeliveredFeature>,
}

/// One feature value the child must be started with, as the declaration
/// spelled it. The two kinds are the two controls a form can draw, so a
/// stored value that is neither — a string on a toggle, an object on a select
/// — has no variant and is refused rather than coerced: the same reading
/// [`feature_is_true`] gives for the tick, generalised.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeliveredFeature {
    /// A toggle's value. `false` asks for nothing on the wire, and the client
    /// that owns the flag decides whether that means a frame is sent or not.
    Toggle { id: String, on: bool },
    /// A select's chosen option id, exactly as the agent declared it.
    Choice { id: String, value: String },
}

impl DeliveredFeature {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Toggle { id, .. } | Self::Choice { id, .. } => id,
        }
    }

    /// What the card prints for this value: a toggle names `on` or `off`, a
    /// select names the option id. One spelling, so the card and the delivery
    /// cannot describe one value two ways.
    pub(crate) fn printed(&self) -> String {
        match self {
            Self::Toggle { on, .. } => on.to_string(),
            Self::Choice { value, .. } => value.clone(),
        }
    }

    /// The stored value, for a client that must check it against what the
    /// provider declares rather than merely forward it. An owned `Value` and
    /// not a reference: the toggle arm would otherwise need a static to point
    /// at, which is a lie about where the value lives.
    pub(crate) fn value(&self) -> Value {
        match self {
            Self::Toggle { on, .. } => Value::Bool(*on),
            Self::Choice { value, .. } => Value::String(value.clone()),
        }
    }
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
            features: Vec::new(),
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
            features: delivered_features(features),
        }
    }
}

/// The stored map as the values a child is started with: every key except the
/// tick, each read through the control that writes it.
///
/// A value that is neither (`autoAccept` aside) is **dropped here, once, for
/// every family**, and the reason is the invariant this struct exists to
/// hold: `check_profile` bounds what can be stored by shape and
/// [`crate::provider_features::prune`] drops what the family does not declare,
/// but a profile written by an older daemon can still hold a key with a value
/// no control produces. Delivering it would be reading a tick out of a string;
/// refusing the whole creation over it would break a profile whose child runs
/// fine today. So the undeliverable value goes and the rest is delivered — and
/// because the card prints this same list, the card cannot name what is not
/// here.
pub(crate) fn delivered_features(
    features: &serde_json::Map<String, Value>,
) -> Vec<DeliveredFeature> {
    let mut delivered = Vec::new();
    for (key, value) in features {
        if key == crate::provider_catalog::AUTO_ACCEPT_FEATURE {
            continue;
        }
        let delivered_value = match value {
            Value::Bool(on) => Some(DeliveredFeature::Toggle {
                id: key.clone(),
                on: *on,
            }),
            Value::String(choice) => Some(DeliveredFeature::Choice {
                id: key.clone(),
                value: choice.clone(),
            }),
            _ => None,
        };
        if let Some(feature) = delivered_value {
            delivered.push(feature);
        }
    }
    delivered
}

/// The one reading of a feature tick: the JSON boolean `true`, and nothing
/// else. A `"true"` string or a number is a value somebody stored, not a
/// tick, and no derivation may read one as a tick.
pub(crate) fn feature_is_true(features: &serde_json::Map<String, Value>, key: &str) -> bool {
    matches!(features.get(key), Some(Value::Bool(true)))
}

/// The refusal every family makes before it applies anything: a stored
/// feature this family has no frame for is not a value to guess at, and a
/// child must not exist that was started without something its card named.
///
/// `declared` is the family's own table ([`crate::provider_features`]), so the
/// check reads the same list the form drew its controls from — one source, and
/// a feature the store prunes can never reach here. It is also the reason a
/// family with no table (a terminal, which receives no delivery) refuses
/// everything: there is no frame for any value.
///
/// A key the table declares but this profile's model does not carry is refused
/// by the *model* arm below rather than dropped: the tick may have been earned
/// when the profile named a different model, and silently starting without it
/// is the silence this whole rule exists to end.
pub(crate) fn refuse_undeclared(
    declared: &[devboule_protocol::VocabularyFeature],
    model: Option<&str>,
    features: &[DeliveredFeature],
) -> Result<(), WireError> {
    for feature in features {
        let Some(declared) = declared.iter().find(|row| row.id == feature.id()) else {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "the profile stores the feature '{}', which this provider offers no control for; the creation is refused rather than started without it",
                    feature.id()
                ),
            ));
        };
        if !declared.offered_on(model) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "the profile stores '{}' for model '{}', which does not carry that feature; the creation is refused rather than started without it",
                    feature.id(),
                    model.unwrap_or("the provider's default"),
                ),
            ));
        }
    }
    Ok(())
}

/// The value one family's own row holds, in the shape that row's control
/// writes. A declared row stored with a value its control cannot produce is
/// refused, not skipped — but [`delivered_features`] already drops those, so
/// reaching here means the two readers disagreed about a type, which is a bug
/// and not a value.
pub(crate) fn toggle_value(
    features: &[DeliveredFeature],
    id: &str,
) -> Result<Option<bool>, WireError> {
    let mut found = None;
    for feature in features {
        if feature.id() != id {
            continue;
        }
        match feature {
            DeliveredFeature::Toggle { on, .. } => found = Some(*on),
            DeliveredFeature::Choice { value, .. } => {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "the profile stores '{id}' as the choice '{value}', which is not a value that control takes; the creation is refused"
                    ),
                ));
            }
        }
    }
    Ok(found)
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

    /// The tick is the boolean `true` and only that: the same reading the
    /// delivery's `auto_accept` constraint is validated by, so the card, the
    /// refusal and the delivery cannot disagree about what a tick is. The
    /// tick is a constraint on which mode is delivered — it is never an input
    /// to the `unattended` marker, which the birth derives from the delivered
    /// mode alone (R2b).
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
