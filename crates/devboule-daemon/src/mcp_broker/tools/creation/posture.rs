//! The permission posture an agent-created child inherits from its creator.
//!
//! One table reads each family's own mode ids into shared postures, and a
//! second spells a posture in the child's family. A family whose modes this
//! table does not read inherits nothing, so the caller must name a mode rather
//! than receive a guessed one.

use devboule_protocol::SessionKind;

/// What a mode lets its session do without asking the person, in terms every
/// family shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Posture {
    /// Nothing asks: the family's own bypass or full-access mode.
    Bypass,
    /// A model reviews the prompts and the person is not asked for them.
    Reviewed,
    /// Edits are approved without asking; other tools may still ask.
    AcceptEdits,
    /// The family's ordinary mode, which asks for its tools.
    Asking,
    /// Plan or read-only: nothing is written.
    ReadOnly,
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

#[cfg(test)]
mod tests {
    use super::inherited_mode;
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
}
