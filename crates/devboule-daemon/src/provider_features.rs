//! The feature declaration: which controls a provider family offers a profile
//! form, and which stored values that form may therefore keep.
//!
//! One rule decides every row in here, and it is the rule the reverted feature
//! editor died for: **a control is offered only for a value the spawn path
//! really applies.** A switch that saves a key no child ever receives is not a
//! disabled feature, it is a lie on a consent surface — the creation card names
//! what the child was started with, and a card that names an undelivered value
//! promises a configuration nobody chose. So each row below names the client
//! that applies it and the frame the value rides. Paseo is the reference for
//! both halves: its `listFeatures` list and its `setFeature`/spawn application
//! are written together, and a feature with no application has no row.
//!
//! Three sources, one per shape of provider:
//!
//! - **Claude and Codex** — a fixed table, as in Paseo's
//!   `feature-definitions.ts` and `codex-feature-definitions.ts`, with no
//!   probe: the daemon authored the launch flag and the turn parameter, so it
//!   knows what it can deliver.
//! - **Pi** — the `autoAccept` tick and nothing else. The permission extension
//!   this family injects is its whole feature surface and the delivered mode
//!   drives it; Paseo's pi returns an empty list for the same reason.
//! - **ACP** — the agent's own declared `select` config options, other than
//!   the model and effort selectors, beside `autoAccept`. That list is only
//!   knowable by asking, so the family is probed ([`crate::acp_client`]) and
//!   the answer cached for the run ([`AcpProbeCache`]).
//!
//! The provider dimension stays open: nothing here is keyed by a provider name,
//! only by the family the catalog resolves it to, and the ACP arm reads its
//! list from the agent rather than from a table that could drift.

use devboule_protocol::{
    VocabularyFeature, VocabularyFeatureControl, VocabularyFeatureOption, VocabularyFeatures,
    VocabularyOrigin, VocabularyState,
};

/// The Claude and Codex fast-inference toggle. Paseo spells the key
/// `fast_mode`; this daemon stores its feature keys in the camelCase the
/// profile's own `features` map already uses (`autoAccept`), so the wire and
/// the stored key cannot disagree about one more spelling.
pub(crate) const FAST_MODE_FEATURE: &str = "fastMode";

/// The label both fast-mode rows carry. Paseo's is `Fast` in both tables.
const FAST_MODE_LABEL: &str = "Fast";

/// The Claude models that carry fast mode. Copied from Paseo's model manifest
/// (`providers/claude/model-manifest.ts`, the entries flagged
/// `supportsFastMode`) — a **fixed table**, because the daemon's own Claude
/// catalog (`claude_catalog.rs`) scrapes model ids and *effort* capabilities
/// out of the CLI bundle and has no fast-mode fact to read. A model absent
/// here gets no toggle, and the launch refuses a stored tick for it rather
/// than deliver one the CLI would ignore.
const CLAUDE_FAST_MODE_MODELS: &[&str] = &[
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-opus-4-8[1m]",
    "claude-opus-4-8",
    "claude-opus-4-7[1m]",
    "claude-opus-4-7",
    "claude-opus-4-6[1m]",
    "claude-opus-4-6",
];

/// The Codex models that carry fast mode — Paseo's
/// `CODEX_FAST_MODE_SUPPORTED_MODELS`, same reason and same consequence.
const CODEX_FAST_MODE_MODELS: &[&str] = &[
    "gpt-6-astra",
    "gpt-5.6",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
    "gpt-5.4",
];

/// The daemon's own row: every agent family that owns a mode knob applies
/// this one, and every one of them refuses a tick that contradicts the
/// delivered mode (`claude_client`/`codex_client`/`pi_client`
/// `tick_contradicts`, `acp_client::apply_profile_delivery`). It is a
/// *constraint on the delivered mode*, not a value handed to the child —
/// [`crate::profile_delivery`] states that shape, and the key is spelled by
/// [`crate::provider_catalog::AUTO_ACCEPT_FEATURE`], the one reader.
fn auto_accept() -> VocabularyFeature {
    VocabularyFeature {
        id: crate::provider_catalog::AUTO_ACCEPT_FEATURE.to_string(),
        label: "Auto accept".to_string(),
        author: VocabularyOrigin::Daemon,
        control: VocabularyFeatureControl::Toggle,
        options: Vec::new(),
        models: None,
    }
}

/// The fast-inference row, gated to the family's own model list.
///
/// Both families apply it on a frame they already send: Claude with the
/// `apply_flag_settings` control request this client writes for the thinking
/// option (`claude_client::send_initial_fast_mode`), Codex with the
/// `serviceTier` parameter of its own `turn/start`
/// (`codex_client::turn_start_params`). Neither substitutes: a value it
/// cannot put on the wire refuses the creation.
///
/// The list is carried **normalised of the `[1m]` suffix** for Claude, because
/// that is how this family compares its own ids everywhere else
/// (`claude_catalog::model_ids_match`): the form then needs no second rule for
/// the suffix, and a profile naming either spelling of a listed model is
/// offered the row and delivered it.
fn fast_mode(models: &[&str]) -> VocabularyFeature {
    VocabularyFeature {
        id: FAST_MODE_FEATURE.to_string(),
        label: FAST_MODE_LABEL.to_string(),
        author: VocabularyOrigin::Daemon,
        control: VocabularyFeatureControl::Toggle,
        options: Vec::new(),
        models: Some(models.iter().map(|id| (*id).to_string()).collect()),
    }
}

/// The Claude table: the tick, and fast mode on the Opus models that carry it.
pub(crate) fn claude_declarations() -> Vec<VocabularyFeature> {
    vec![auto_accept(), fast_mode(CLAUDE_FAST_MODE_MODELS)]
}

/// The Codex table: the tick, and fast mode on the listed models.
///
/// Paseo's Codex table also carries `plan_mode`. It has **no row here**, for
/// the rule at the head of this file: Paseo applies plan mode by picking a
/// `collaborationMode` named `plan` out of the list the app-server reports,
/// and this daemon's Codex client reads no such list and sends no such
/// parameter — its mode vocabulary (`codex_view::CODEX_MODES`) is an
/// approval-policy and sandbox table. Offering `planMode` would save a key no
/// frame carries.
pub(crate) fn codex_declarations() -> Vec<VocabularyFeature> {
    vec![auto_accept(), fast_mode(CODEX_FAST_MODE_MODELS)]
}

/// The Pi table: the tick alone. Paseo's pi client returns an empty list;
/// this daemon has one thing a pi profile can genuinely ask for, and its
/// client enforces it, so it is declared.
pub(crate) fn pi_declarations() -> Vec<VocabularyFeature> {
    vec![auto_accept()]
}

/// The ACP list: the daemon's own tick beside what the agent declared, and
/// the two rows differ in `author` because they differ in who knows them.
pub(crate) fn acp_declarations(probed: Vec<VocabularyFeature>) -> Vec<VocabularyFeature> {
    [vec![auto_accept()], probed].concat()
}

/// A terminal has no feature surface: no mode, no permission mechanism, no
/// delivery at all (`TerminalProvider::validate_delivery` delivers nothing).
pub(crate) fn terminal_declarations() -> Vec<VocabularyFeature> {
    Vec::new()
}

/// The axis for a family whose list is known without asking anyone.
fn answered(items: Vec<VocabularyFeature>) -> VocabularyFeatures {
    VocabularyFeatures::new(
        if items.is_empty() {
            VocabularyState::None
        } else {
            VocabularyState::Present
        },
        items,
    )
    .expect("the state above always fits the list")
}

pub(crate) fn claude_axis() -> VocabularyFeatures {
    answered(claude_declarations())
}

pub(crate) fn codex_axis() -> VocabularyFeatures {
    answered(codex_declarations())
}

pub(crate) fn pi_axis() -> VocabularyFeatures {
    answered(pi_declarations())
}

pub(crate) fn terminal_axis() -> VocabularyFeatures {
    answered(terminal_declarations())
}

/// The ACP axis from a probe answer that has arrived. An empty probed list is
/// `present` with the tick, not `none`: the family does offer one feature.
pub(crate) fn acp_axis(probed: Vec<VocabularyFeature>) -> VocabularyFeatures {
    answered(acp_declarations(probed))
}

/// The axis for a read that is running: the form draws "checking…" from this
/// and asks again, and nothing about the profile changes in the meantime.
pub(crate) fn probing_axis() -> VocabularyFeatures {
    VocabularyFeatures::probing()
}

/// The axis for a read that could not be made: the agent is not installed, or
/// its own startup failed. `absent`, not `none` — "nobody could ask" and "the
/// provider has nothing" are different facts, and the second one would tell
/// the human to stop looking.
pub(crate) fn unavailable_axis() -> VocabularyFeatures {
    VocabularyFeatures::new(VocabularyState::Absent, Vec::new())
        .expect("an absent axis carries no items")
}

/// Whether one model id, compared the way this family compares its own ids
/// (suffix-tolerant: the CLI's `claude-opus-5-20260101[1m]` is the same model
/// as the profile's `claude-opus-5-20260101`), is inside a row's gate.
///
/// The comparison is the family's own, so a delivery and a form agree about
/// which spelling of a model id carries a feature: [`VocabularyFeature::models`]
/// stores the gate and this reads it the way the client that applies the value
/// would.
///
/// The `[1m]` suffix is stripped from **both** sides here, because the stored
/// gate carries it and a profile may name either spelling; the form answers
/// from [`VocabularyFeature::offered_on`] alone and needs no second rule, since
/// a gated declaration only ever reaches a client that reads it through here.
pub(crate) fn offered_for(row: &VocabularyFeature, model_id: &str) -> bool {
    let model_id = model_id.strip_suffix("[1m]").unwrap_or(model_id);
    match &row.models {
        None => true,
        Some(models) => models
            .iter()
            .any(|listed| listed.strip_suffix("[1m]").unwrap_or(listed) == model_id),
    }
}

/// Whether a stored value fits the control that writes it: a toggle holds a
/// JSON boolean, a select one of its declared option ids. Stated once because
/// the save path and the spawn path both need it and must not disagree about
/// what a legal value is.
///
/// `false` on a toggle is legal and ordinary — the form writes the tick off by
/// storing `false` — so only a value the control could never produce is
/// refused: a `"true"` string, a number, an object.
pub(crate) fn value_fits(feature: &VocabularyFeature, value: &serde_json::Value) -> bool {
    match feature.control {
        VocabularyFeatureControl::Toggle => value.is_boolean(),
        VocabularyFeatureControl::Select => value
            .as_str()
            .is_some_and(|value| feature.options.iter().any(|option| option.id == value)),
    }
}

/// The stored `features` map as the family can apply it: a key the declaration
/// does not carry is dropped, and so is a declared key whose stored value is
/// not one its control could produce. The caps in `agent_profiles.rs` bound
/// what arrives first, and the count cap cannot be reached by a pruned map
/// because pruning only ever removes entries.
///
/// Dropping rather than refusing is the brief's `D4` and Paseo's
/// `pruneFeatureValues`, and it is the store's own discipline: a document read
/// back after a provider update is the human's work, and a value that was
/// never going to reach a child is not a value to quarantine the file over.
/// The rule runs on both roads — `check_document` is shared by `load` and
/// `set` — so no document can be in the store holding a key nothing delivers.
///
/// The gate is applied on the *key*, not the model: a feature the family
/// declares on other models survives here and is refused at spawn, because a
/// profile's model is editable without touching its features, and dropping a
/// tick the human can get back by naming the model again is a worse surprise
/// than one honest refusal at creation.
pub(crate) fn prune(
    declared: &[VocabularyFeature],
    features: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut kept = serde_json::Map::new();
    for feature in declared {
        let Some(value) = features.get(&feature.id) else {
            continue;
        };
        if value_fits(feature, value) {
            kept.insert(feature.id.clone(), value.clone());
        }
    }
    kept
}

/// The stored map pruned for one provider family. `Err` says the provider has
/// no declaration to prune against — an ACP provider whose probe has not
/// answered yet — and the caller must then **leave the map alone**: an unknown
/// list is not an empty one, and pruning against nothing would delete a
/// provider-authored value the daemon simply has not learned about yet.
pub(crate) fn prune_for(
    family: devboule_protocol::SessionKind,
    features: &serde_json::Map<String, serde_json::Value>,
    probed: Option<Vec<VocabularyFeature>>,
) -> Result<serde_json::Map<String, serde_json::Value>, ()> {
    let declared = match family {
        devboule_protocol::SessionKind::Claude => claude_declarations(),
        devboule_protocol::SessionKind::Codex => codex_declarations(),
        devboule_protocol::SessionKind::Pi => pi_declarations(),
        devboule_protocol::SessionKind::Terminal => terminal_declarations(),
        // The open dimension: the agent's own list, or nothing yet.
        devboule_protocol::SessionKind::Acp => match probed {
            Some(probed) => acp_declarations(probed),
            None => return Err(()),
        },
    };
    Ok(prune(&declared, features))
}

/// A select read off an agent's own declared config option: the option's id is
/// the stored key, its `name` the label, its choices the values the wire
/// takes.
pub(crate) fn select_declaration(
    id: String,
    label: String,
    options: Vec<VocabularyFeatureOption>,
) -> Option<VocabularyFeature> {
    if options.is_empty() {
        // A select with no choices is a control that cannot be drawn and a
        // value that cannot be validated: the agent declared the *shape* of a
        // dial and no positions for it.
        return None;
    }
    Some(VocabularyFeature {
        id,
        label,
        author: VocabularyOrigin::Provider,
        control: VocabularyFeatureControl::Select,
        options,
        models: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feature(id: &str) -> VocabularyFeature {
        VocabularyFeature {
            id: id.to_string(),
            label: id.to_string(),
            author: VocabularyOrigin::Provider,
            control: VocabularyFeatureControl::Select,
            options: vec![
                VocabularyFeatureOption {
                    id: "a".to_string(),
                    label: "A".to_string(),
                },
                VocabularyFeatureOption {
                    id: "b".to_string(),
                    label: "B".to_string(),
                },
            ],
            models: None,
        }
    }

    /// The declared ids of a family, as owned strings: the tables borrow their
    /// ids from `&'static` rows, and a `Vec<&str>` taken from a local
    /// declaration list would outlive the list it was read out of.
    fn ids(declared: Vec<VocabularyFeature>) -> Vec<String> {
        declared.into_iter().map(|feature| feature.id).collect()
    }

    fn map(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().expect("object").clone()
    }

    /// Every agent family declares the tick it actually applies, and the two
    /// fixed tables add the fast row they can put on the wire. Nothing here
    /// carries a feature no client reads: that is the defect the module header
    /// names, and a test that added an undelivered row would have to name the
    /// client that applies it before this list would let it.
    #[test]
    fn each_family_declares_only_what_its_client_applies() {
        let tick = crate::provider_catalog::AUTO_ACCEPT_FEATURE;
        assert_eq!(ids(pi_declarations()), [tick.to_string()]);
        assert_eq!(ids(terminal_declarations()), Vec::<String>::new());
        assert_eq!(
            ids(claude_declarations()),
            [tick.to_string(), FAST_MODE_FEATURE.to_string()]
        );
        assert_eq!(
            ids(codex_declarations()),
            [tick.to_string(), FAST_MODE_FEATURE.to_string()]
        );
        assert_eq!(
            ids(acp_declarations(vec![feature("engine")])),
            [tick.to_string(), "engine".to_string()]
        );
    }

    /// The fast row is gated to a model list and the tick is not gated at all:
    /// a control that appears only on the models that carry the flag, and one
    /// that is always there because every family enforces it. The gate is read
    /// through [`offered_for`], which tolerates the CLI's `[1m]` suffix the
    /// same way the family compares its own model ids everywhere else.
    #[test]
    fn the_fast_row_is_gated_and_the_tick_is_not() {
        let gated = fast_mode(CLAUDE_FAST_MODE_MODELS);
        assert!(offered_for(&gated, "claude-opus-5"));
        assert!(offered_for(&gated, "claude-opus-4-8"));
        assert!(
            offered_for(&gated, "claude-opus-4-8[1m]"),
            "the suffix spelling of a listed model is the same model"
        );
        assert!(
            !offered_for(&gated, "claude-sonnet-5"),
            "a model outside the table gets no fast row"
        );
        assert!(
            !gated.offered_on(None),
            "no model chosen is no model known to support it"
        );
        assert!(auto_accept().offered_on(None));
        assert!(auto_accept().offered_on(Some("anything")));
        // The Codex gate is exact: its ids come from the provider's own
        // `model/list`, with no alias spelling to tolerate.
        let codex_gated = fast_mode(CODEX_FAST_MODE_MODELS);
        assert!(offered_for(&codex_gated, "gpt-5.6-luna"));
        assert!(
            offered_for(&codex_gated, "gpt-5.6-luna[1m]"),
            "the suffix is stripped on both sides for every family: the              comparison is one rule, and a gate that is exact for one family and              tolerant for another is two rules a caller can mix up"
        );
        assert!(!offered_for(&codex_gated, "gpt-4-others"));
    }

    /// Pruning is by key: a stored key the family does not declare goes, and a
    /// declared key stays whatever model the profile happens to name. The
    /// model gate refuses at spawn, it does not silently delete here — the
    /// `prune` doc states why.
    #[test]
    fn an_undeclared_key_is_dropped_and_a_declared_one_kept() {
        let declared = claude_declarations();
        let stored = map(json!({
            "autoAccept": true,
            "fastMode": true,
            "sandbox": "none",
            "retries": 3,
        }));
        let kept = prune(&declared, &stored);
        assert_eq!(
            kept.keys().cloned().collect::<Vec<_>>(),
            vec![
                crate::provider_catalog::AUTO_ACCEPT_FEATURE.to_string(),
                FAST_MODE_FEATURE.to_string()
            ],
            "the two declared keys, in declaration order, and nothing else"
        );
        assert_eq!(
            kept.values().count(),
            2,
            "and the two undeclared keys are gone, not stored as null: {kept:?}"
        );
    }

    /// A declared key whose value the control could never produce is dropped
    /// with the same rule that drops the key: `"true"` on a toggle is a value
    /// somebody stored, not a value the checkbox writes, and delivering it
    /// would read a tick out of a string.
    #[test]
    fn a_value_the_control_cannot_produce_is_dropped_too() {
        let declared = claude_declarations();
        let kept = prune(
            &declared,
            &map(json!({
                "autoAccept": "true",
                "fastMode": json!({"on": true}),
            })),
        );
        assert!(kept.is_empty(), "a string tick and an object flag both go");

        // A select takes exactly its declared choices.
        let engine = feature("engine");
        assert!(value_fits(&engine, &json!("a")));
        assert!(
            !value_fits(&engine, &json!("c")),
            "outside the declared list"
        );
        assert!(!value_fits(&engine, &json!(true)), "not a choice at all");
        let empty_choice = VocabularyFeature {
            options: vec![
                VocabularyFeatureOption {
                    id: "".to_string(),
                    label: "Use default".to_string(),
                },
                VocabularyFeatureOption {
                    id: "a".to_string(),
                    label: "A".to_string(),
                },
            ],
            ..engine
        };
        assert!(value_fits(&empty_choice, &json!("")));
    }

    /// An ACP provider nobody has read yet has **no** list, which is not the
    /// same fact as an empty one: pruning against nothing would delete a
    /// provider-authored key the daemon simply has not learned about.
    #[test]
    fn an_unread_acp_provider_is_not_pruned() {
        let stored = map(json!({"engine": "a"}));
        assert!(
            prune_for(devboule_protocol::SessionKind::Acp, &stored, None).is_err(),
            "no probe answer means no prune"
        );
        let pruned = prune_for(
            devboule_protocol::SessionKind::Acp,
            &stored,
            Some(vec![feature("engine")]),
        )
        .expect("a read answer prunes");
        assert_eq!(
            pruned["engine"],
            serde_json::Value::String("a".to_string()),
            "a declared key with a declared choice survives the prune"
        );

        // And a key the read did not find goes, once the read has answered.
        let pruned = prune_for(
            devboule_protocol::SessionKind::Acp,
            &map(json!({"autoAccept": true, "gone": "x"})),
            Some(vec![]),
        )
        .expect("a read answer prunes");
        assert_eq!(
            pruned.keys().cloned().collect::<Vec<_>>(),
            vec![crate::provider_catalog::AUTO_ACCEPT_FEATURE.to_string()]
        );
    }

    /// The axis states the tick/none/absent distinction the form draws three
    /// sentences from: a family with a table is `present`, a terminal with no
    /// surface is `none`, a read that could not run is `absent` — and a
    /// provider that has not been read yet is `absent` with `probing`.
    #[test]
    fn the_axis_keeps_present_none_and_absent_three_apart() {
        assert_eq!(claude_axis().state, VocabularyState::Present);
        assert_eq!(terminal_axis().state, VocabularyState::None);
        assert_eq!(unavailable_axis().state, VocabularyState::Absent);
        assert!(!unavailable_axis().probing);
        assert!(probing_axis().probing);
        // An ACP agent that declared no extra option still offers the tick, so
        // its axis is `present` and not `none`.
        assert_eq!(acp_axis(vec![]).state, VocabularyState::Present);
    }
}
