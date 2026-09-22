//! The machinery both workspace-git slices share: classify one workspace
//! folder for git, run git there under the house's timeouts and caps, and
//! speak about failures without leaking a path or git's stderr. Extracted in
//! the slice-2 round so the status list and the file diff read the same
//! folder and say the same sentences — two copies of a sentence the panel
//! shows would be two truths.

use std::path::Path;

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
const NOT_A_DIRECTORY: &str = "the workspace folder is not a directory";
const PROBE_TIMEOUT: &str = "git did not answer within the probe timeout";
const GIT_UNAVAILABLE: &str = "git could not be run";

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
