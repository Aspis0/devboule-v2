//! The machinery the workspace slices share: confine a requested path to one
//! workspace folder without following any link, classify that folder for git,
//! run git there under the house's timeouts and caps, and speak about failures
//! without leaking a path or git's stderr. Extracted in the slice-2 round so
//! the status list and the file diff read the same folder and say the same
//! sentences, and widened in the slice-4 round so the Files tree refuses a
//! path with those same sentences — two copies of a sentence a panel shows
//! would be two truths.

use std::path::{Component, Path, PathBuf};

use crate::git::{
    detect_git_repository, run_git_args, GitOutput, GitRepositoryStatus, GitRunError,
};

/// A workspace folder below a repository root. Read from a subdirectory,
/// `git status` and `git diff` answer for the whole repository with paths
/// relative to that subdirectory, so the panel would show files of other
/// checkouts. Decided in the slice-1 fix round: refuse and say what the
/// panel would have shown instead of resolving the top level, which is a
/// product choice these slices do not make.
pub(crate) const INSIDE_A_REPOSITORY: &str =
    "this workspace folder is inside a git repository but is not its root; the Changes panel \
     lists changes of the repository, not of this folder";

/// No path in any sentence below: `error` on these frames crosses a wire
/// whose redaction seam does not touch it (the debt recorded on
/// `WorkspaceGitStatus`), so the message itself is the guard.
const NOT_A_REPOSITORY: &str = "this workspace folder is not a git repository";
pub(crate) const NOT_A_DIRECTORY: &str = "the workspace folder is not a directory";
const PROBE_TIMEOUT: &str = "git did not answer within the probe timeout";
const GIT_UNAVAILABLE: &str = "git could not be run";

/// Refusals of a requested path, shared by every slice that confines one:
/// the diff refuses with these words and the Files tree refuses with the
/// same, so one frame never names a path the other one hides. None of these
/// sentences contains an absolute path — `error` travels on a wire whose
/// redaction seam does not touch these frames.
pub(crate) const OUTSIDE_THE_WORKSPACE: &str = "the requested path is outside the workspace folder";
pub(crate) const LINK_FINAL: &str = "the requested path is a symbolic link; its target is not read";
const LINK_CROSSED: &str = "the requested path crosses a link and is not read";

/// What one workspace folder answers when asked whether git can serve it.
pub(crate) enum Probe {
    Ready,
    NotRepository,
    InsideRepository,
    /// Not a directory, a probe git did not answer, or git itself missing —
    /// with the pathless sentence that says which.
    Refused(&'static str),
}

/// Classify `root` once, ahead of any git command: the folder must exist as
/// a directory first (which also covers a workspace whose folder vanished
/// after the registry had cached it), then git's own probe decides.
pub(crate) fn probe(root: &Path) -> Probe {
    if !root.is_dir() {
        return Probe::Refused(NOT_A_DIRECTORY);
    }
    match detect_git_repository(root) {
        GitRepositoryStatus::RepositoryRoot => Probe::Ready,
        GitRepositoryStatus::InsideRepository => Probe::InsideRepository,
        GitRepositoryStatus::NotRepository => Probe::NotRepository,
        GitRepositoryStatus::TimedOut => Probe::Refused(PROBE_TIMEOUT),
        GitRepositoryStatus::Unknown => Probe::Refused(GIT_UNAVAILABLE),
    }
}

impl Probe {
    /// The pathless sentence for every answer that is not `Ready`, `None`
    /// only for `Ready`. Callers that answer a refusal with one word use it
    /// directly; the status list maps the variants itself, because for it
    /// "not a repository" is an answer (`is_git: false`, `error: null`) and
    /// not a failure.
    pub(crate) fn refusal(self) -> Option<&'static str> {
        match self {
            Probe::Ready => None,
            Probe::NotRepository => Some(NOT_A_REPOSITORY),
            Probe::InsideRepository => Some(INSIDE_A_REPOSITORY),
            Probe::Refused(message) => Some(message),
        }
    }
}

/// Run `git` in `root` with a closed argv — this house's runner, which
/// brings its own timeouts, stdout cap and Job Object; no shell, and the
/// subcommand is a slice of `&str`, never a formatted string.
pub(crate) fn git(root: &Path, subcommand: &[&str]) -> Result<GitOutput, GitRunError> {
    let mut arguments = vec!["-C".to_string(), root.to_string_lossy().into_owned()];
    arguments.extend(subcommand.iter().map(|argument| (*argument).to_string()));
    run_git_args(&arguments)
}

/// A git that never started, timed out or is not installed — named by
/// category, never by the OS error, which carries paths.
pub(crate) fn run_error(error: GitRunError, operation: &str) -> String {
    match error {
        GitRunError::NotFound => format!("{operation}: git is not installed"),
        GitRunError::TimedOut => format!("{operation}: git timed out"),
        GitRunError::SpawnFailed => format!("{operation}: git could not be started"),
    }
}

/// A failed command's identity and exit code, and never its stderr: git
/// writes absolute paths and personal file names into stderr, and `error`
/// travels on a wire whose redaction seam does not touch these frames. The
/// detail is dropped on purpose, not lost by accident — a local debug
/// session that needs it should print `output.stderr` at the call site.
pub(crate) fn exit_error(operation: &str, output: &GitOutput) -> String {
    match output.code {
        Some(code) => format!("{operation} exited with code {code}"),
        None => format!("{operation} was terminated before it could report a code"),
    }
}

/// The requested path, confined: non-empty, relative (no root, no prefix,
/// matched by component rather than by `is_absolute()`, which on Windows
/// calls a bare `/x` relative), no `..`, and inside `root` once joined.
/// The last check restates what the component rules guarantee rather than
/// trusting them. This trusts the spelling only; [`walk`] is the half that
/// trusts the filesystem.
pub(crate) fn confined(root: &Path, requested: &str) -> Option<PathBuf> {
    if requested.is_empty() {
        return None;
    }
    for component in Path::new(requested).components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    let joined = root.join(requested);
    joined.starts_with(root).then_some(joined)
}

/// What the filesystem walk of a confined path found.
pub(crate) enum Walked {
    /// Every component statted without following, none a link; carries the
    /// final component's metadata.
    Inside(std::fs::Metadata),
    /// A component has no stat — a deletion or a name that never existed;
    /// nothing below it exists either.
    Missing,
    /// A component is a link: an intermediate one would resolve outside the
    /// workspace, and the final one's target is not read at all. The
    /// sentence says which.
    Link(&'static str),
}

/// Component by component, without following: an intermediate symlink or
/// junction would resolve outside the workspace — measured with a real,
/// reachable junction in `workspace_git_diff_without_lines_tests.rs` — and
/// the final component may not be a link either, because its target is not
/// read. Callers pass only a path [`confined`] accepted, and only non-empty:
/// an empty request is the folder itself, which the listing names before it
/// walks. A link swapped in between these stats and the caller's open is the
/// stat→open race, declared with slice 1's stat→read one: no test holds a
/// swapper still for the read, which still opens the path by name after
/// this returns. The preview stage closed its half differently — it does
/// not reopen the name: it proves the swap away on its own handle and holds
/// the swapper still with two seam tests
/// (`crate::workspace_file_preview::verified_source`).
pub(crate) fn walk(root: &Path, requested: &str) -> Walked {
    let components: Vec<Component> = Path::new(requested)
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect();
    let mut prefix = root.to_path_buf();
    let last_component = components.len().saturating_sub(1);
    for (index, component) in components.iter().enumerate() {
        prefix.push(*component);
        let Ok(metadata) = std::fs::symlink_metadata(&prefix) else {
            return Walked::Missing;
        };
        if crosses_a_link(&metadata) {
            return Walked::Link(if index == last_component {
                LINK_FINAL
            } else {
                LINK_CROSSED
            });
        }
        if index == last_component {
            return Walked::Inside(metadata);
        }
    }
    // No components: only reachable when a caller forgot its own precondition.
    Walked::Missing
}

/// Whether this stat describes a link rather than an ordinary entry. Shared
/// because the rule must hold everywhere a path is read: the walk refuses a
/// link's *target*, and the listing refuses to classify the link itself.
pub(crate) fn crosses_a_link(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink() || is_reparse_point(metadata)
}

/// Windows: this half of the refusal does not rest on how a toolchain
/// labels a junction, because that label has been measured twice on the
/// same `mklink /J` target with opposite answers: slice 2 recorded lstat
/// calling one a directory (not `is_symlink`), while this round measures
/// `is_symlink() = true, is_dir() = false`, attributes `0x410`. Every
/// link-like reparse point carries `FILE_ATTRIBUTE_REPARSE_POINT` (0x400),
/// so a walk of ordinary components stays inside an ordinary root whichever
/// label wins — and dropping this arm is measurably a no-op today (mutation
/// `m:f` half-survived: only the label half kept every test alive), which is
/// why it stays: declared redundancy over a fact that has flipped once.
#[cfg(windows)]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

/// Off Windows every link is a symlink, which [`crosses_a_link`] saw.
#[cfg(not(windows))]
fn is_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}
