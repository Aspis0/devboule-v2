//! The permission posture an agent-created child inherits from its creator.
//!
//! One table reads each family's own mode ids into shared postures, and a
//! second spells a posture in the child's family. A family whose modes this
//! table does not read inherits nothing, so the caller must name a mode rather
//! than receive a guessed one.

use devboule_protocol::SessionKind;

/// What a mode lets its session do without asking the person, in terms every
/// family shares. Declared from the most restrictive to the most permissive, so
/// the derived order is the rank a child may not exceed its creator's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Posture {
    /// Plan or read-only: nothing is written.
    ReadOnly,
    /// The family's ordinary mode, which asks for its tools.
    Asking,
    /// Edits are approved without asking; other tools may still ask.
    AcceptEdits,
    /// A model reviews the prompts and the person is not asked for them.
    Reviewed,
    /// Nothing asks: the family's own bypass or full-access mode.
    Bypass,
}

/// The mode an agent-created child starts in, and the mode the agent asked for
/// when that one was replaced.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ChildMode {
    pub(super) mode: String,
    pub(super) replaced: Option<String>,
}

/// The posture a mode of `kind` stands for, or `None` for a mode this table does
/// not read.
fn posture_of(kind: &SessionKind, mode_id: &str) -> Option<Posture> {
    use Posture::{AcceptEdits, Asking, Bypass, ReadOnly, Reviewed};
    match (kind, mode_id) {
        (SessionKind::Claude, "bypassPermissions")
        | (SessionKind::Codex, "full-access")
        | (SessionKind::Pi, "bypass") => Some(Bypass),
        (SessionKind::Claude, "auto") | (SessionKind::Codex, "auto-review") => Some(Reviewed),
        (SessionKind::Claude, "acceptEdits") => Some(AcceptEdits),
        (SessionKind::Claude, "default")
        | (SessionKind::Codex, "auto")
        | (SessionKind::Pi, "ask") => Some(Asking),
        (SessionKind::Claude, "plan") | (SessionKind::Codex, "read-only") => Some(ReadOnly),
        _ => None,
    }
}

/// The mode a child of `kind` starts in to hold `posture`, or `None` when the
/// family has no mode for it.
fn child_mode(kind: &SessionKind, posture: Posture) -> Option<&'static str> {
    use Posture::{AcceptEdits, Asking, Bypass, ReadOnly, Reviewed};
    match (kind, posture) {
        (SessionKind::Claude, Bypass) => Some("bypassPermissions"),
        (SessionKind::Claude, Reviewed) => Some("auto"),
        (SessionKind::Claude, AcceptEdits) => Some("acceptEdits"),
        (SessionKind::Claude, Asking) => Some("default"),
        (SessionKind::Claude, ReadOnly) => Some("plan"),
        (SessionKind::Codex, Bypass) => Some("full-access"),
        (SessionKind::Codex, Reviewed) => Some("auto-review"),
        (SessionKind::Codex, Asking) => Some("auto"),
        (SessionKind::Codex, ReadOnly) => Some("read-only"),
        // Pi has no classifier. `bypass` is its only mode that does not ask, so
        // a reviewed creator's child is not asked for anything either.
        (SessionKind::Pi, Bypass | Reviewed) => Some("bypass"),
        (SessionKind::Pi, Asking) => Some("ask"),
        _ => None,
    }
}

/// The mode an agent-created child of `child` starts in when the caller names
/// none: the creator's posture, spelled in the child's family. `creator` is the
/// creator's family and current mode, and `None` when the creator has neither.
pub(super) fn inherited_mode(
    creator: Option<(&SessionKind, &str)>,
    child: &SessionKind,
) -> Option<&'static str> {
    let (kind, mode_id) = creator?;
    child_mode(child, posture_of(kind, mode_id)?)
}

/// Whether a creator in this mode asks the person before it creates a child. A
/// bypass, reviewed or accept-edits creator creates on its own; a mode this table
/// does not read still asks.
pub(super) fn creation_asks_person(kind: &SessionKind, mode_id: &str) -> bool {
    !matches!(
        posture_of(kind, mode_id),
        Some(Posture::Bypass | Posture::Reviewed | Posture::AcceptEdits)
    )
}

/// The mode an agent-created child starts in. A child never asks the person through
/// its mode and never runs wider than its creator, so an asking or wider request is
/// replaced by the creator's posture. Plan and read-only only restrict, so they stand.
pub(super) fn agent_child_mode(
    child: &SessionKind,
    requested: Option<&str>,
    creator: Option<(&SessionKind, &str)>,
) -> Result<ChildMode, String> {
    // A family the table does not read is the same family as its creator: the
    // child then runs in the mode the creator runs in, as Paseo does for a
    // same-provider child.
    let inherited = inherited_mode(creator, child)
        .map(str::to_string)
        .or_else(|| {
            creator
                .filter(|(kind, _)| *kind == child)
                .map(|(_, mode_id)| mode_id.to_string())
        });
    if let Some(requested) = requested {
        let own = creator.and_then(|(kind, mode_id)| posture_of(kind, mode_id));
        let kept = match posture_of(child, requested) {
            Some(Posture::ReadOnly) => true,
            Some(posture) => posture != Posture::Asking && own.is_some_and(|own| posture <= own),
            None => false,
        };
        if kept {
            return Ok(ChildMode {
                mode: requested.to_string(),
                replaced: None,
            });
        }
        let mode = inherited.ok_or_else(|| {
            format!(
                "mode {requested} would let the child ask you or run wider than you, and your own mode does not carry over to this provider; name a read-only mode or a profile"
            )
        })?;
        let replaced = (mode != requested).then(|| requested.to_string());
        return Ok(ChildMode { mode, replaced });
    }
    let mode = inherited.ok_or_else(|| {
        "mode is required: the creating agent's mode does not carry over to this provider"
            .to_string()
    })?;
    Ok(ChildMode {
        mode,
        replaced: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{agent_child_mode, inherited_mode};
    use devboule_protocol::SessionKind;
    use devboule_protocol::SessionKind::{Acp, Claude, Codex, Pi};

    /// Every creator family and mode this table reads, against each child family.
    /// A `None` row is a pair the table refuses: the caller must name a mode.
    #[test]
    fn a_child_inherits_the_creators_posture_in_its_own_spelling() {
        let cases: &[(SessionKind, &str, SessionKind, Option<&str>)] = &[
            (Codex, "auto-review", Claude, Some("auto")),
            (Codex, "full-access", Claude, Some("bypassPermissions")),
            (Codex, "read-only", Claude, Some("plan")),
            (Codex, "auto", Claude, Some("default")),
            (Codex, "auto-review", Codex, Some("auto-review")),
            (Codex, "full-access", Codex, Some("full-access")),
            (Codex, "read-only", Codex, Some("read-only")),
            (Codex, "auto", Codex, Some("auto")),
            (Codex, "full-access", Pi, Some("bypass")),
            (Codex, "auto-review", Pi, Some("bypass")),
            (Codex, "auto", Pi, Some("ask")),
            (Codex, "auto", Acp, None),
            (Claude, "default", Claude, Some("default")),
            (Claude, "acceptEdits", Claude, Some("acceptEdits")),
            (Claude, "auto", Claude, Some("auto")),
            (Claude, "plan", Claude, Some("plan")),
            (
                Claude,
                "bypassPermissions",
                Claude,
                Some("bypassPermissions"),
            ),
            (Claude, "bypassPermissions", Codex, Some("full-access")),
            (Claude, "auto", Codex, Some("auto-review")),
            (Claude, "acceptEdits", Codex, None),
            (Claude, "default", Codex, Some("auto")),
            (Claude, "plan", Codex, Some("read-only")),
            (Claude, "bypassPermissions", Pi, Some("bypass")),
            (Claude, "auto", Pi, Some("bypass")),
            (Claude, "acceptEdits", Pi, None),
            (Claude, "default", Pi, Some("ask")),
            (Claude, "default", Acp, None),
            (Pi, "bypass", Claude, Some("bypassPermissions")),
            (Pi, "bypass", Codex, Some("full-access")),
            (Pi, "bypass", Pi, Some("bypass")),
            (Pi, "ask", Claude, Some("default")),
            (Pi, "ask", Codex, Some("auto")),
            (Pi, "ask", Pi, Some("ask")),
            (Pi, "bypass", Acp, None),
            (Acp, "auto_accept", Claude, None),
            (Acp, "anything", Codex, None),
        ];
        for (creator, mode, child, expected) in cases {
            assert_eq!(
                inherited_mode(Some((creator, *mode)), child),
                *expected,
                "{creator:?} in {mode} creating {child:?}"
            );
        }
    }

    #[test]
    fn a_creator_with_no_kind_or_mode_passes_nothing_on() {
        assert_eq!(inherited_mode(None, &Claude), None);
    }

    /// The mode an agent's child starts in, from the mode the agent asked for and its
    /// creator's posture: `(mode, replaced)`, or `None` for a refusal.
    #[test]
    fn an_agent_child_never_asks_and_never_outranks_its_creator() {
        type Case<'a> = (
            SessionKind,
            &'a str,
            Option<(SessionKind, &'a str)>,
            Option<(&'a str, Option<&'a str>)>,
        );
        let cases: &[Case] = &[
            (
                Claude,
                "default",
                Some((Codex, "auto-review")),
                Some(("auto", Some("default"))),
            ),
            (
                Claude,
                "plan",
                Some((Codex, "auto-review")),
                Some(("plan", None)),
            ),
            (
                Claude,
                "bypassPermissions",
                Some((Claude, "auto")),
                Some(("auto", Some("bypassPermissions"))),
            ),
            (
                Claude,
                "acceptEdits",
                Some((Claude, "bypassPermissions")),
                Some(("acceptEdits", None)),
            ),
            (
                Claude,
                "acceptEdits",
                Some((Claude, "default")),
                Some(("default", Some("acceptEdits"))),
            ),
            (
                Claude,
                "default",
                Some((Codex, "read-only")),
                Some(("plan", Some("default"))),
            ),
            (
                Claude,
                "default",
                Some((Claude, "default")),
                Some(("default", None)),
            ),
            (
                Claude,
                "no-such-mode",
                Some((Codex, "auto-review")),
                Some(("auto", Some("no-such-mode"))),
            ),
            (Claude, "plan", None, Some(("plan", None))),
            (Claude, "default", None, None),
            (
                Codex,
                "auto",
                Some((Claude, "plan")),
                Some(("read-only", Some("auto"))),
            ),
            (
                Pi,
                "ask",
                Some((Codex, "auto-review")),
                Some(("bypass", Some("ask"))),
            ),
        ];
        for (child, requested, creator, expected) in cases {
            let creator = creator.as_ref().map(|(kind, mode)| (kind, *mode));
            let got = agent_child_mode(child, Some(requested), creator)
                .ok()
                .map(|chosen| (chosen.mode, chosen.replaced));
            let expected =
                expected.map(|(mode, replaced)| (mode.to_string(), replaced.map(str::to_string)));
            assert_eq!(
                got, expected,
                "{child:?} asked for {requested} under {creator:?}"
            );
        }
    }
}
