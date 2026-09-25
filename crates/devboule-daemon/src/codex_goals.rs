//! The Codex goals version gate: whether the binary on this machine has the
//! experimental `goals` feature, what that adds to the launch line, and what it
//! adds to the menu.
//!
//! Translated from
//! `paseo-src/packages/server/src/server/agent/providers/codex-app-server-agent.ts`:
//! `CODEX_GOALS_MIN_VERSION` :170, `parseCodexVersion` :173-178,
//! `codexVersionAtLeast` :180-191, `resolveGoalsEnabled` :7031-7053 and the
//! launcher's `--enable goals` :7089-7091.
//!
//! Both halves are one decision, and it is made once: an older binary gets
//! neither the flag nor the `goal` menu entry, because the flag is the only
//! thing that turns the `thread/goal/*` requests on.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

/// `CODEX_GOALS_MIN_VERSION` :170. Below it Codex rejects `--enable goals` at
/// launch, so the flag must not be passed at all.
const CODEX_GOALS_MIN_VERSION: [u64; 3] = [0, 128, 0];

/// How long one `--version` probe may take. Paseo's is 5 s
/// (`diagnostic-utils.ts:138-147`), and a probe that outlives it reads as a
/// binary without the feature.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The answer of the version gate.
#[derive(Clone, Copy)]
pub(crate) struct Goals(bool);

impl Goals {
    pub(crate) fn enabled(self) -> bool {
        self.0
    }

    /// The arguments the launcher adds when the gate passed
    /// (`spawnAppServer` :7089-7091). None when it did not: an older Codex
    /// refuses the flag outright and the session would not start.
    pub(crate) fn launch_args(self) -> &'static [&'static str] {
        if self.0 {
            &["--enable", "goals"]
        } else {
            &[]
        }
    }

    /// The gate from a `--version` answer already in hand — the half that is
    /// worth testing without spawning anything.
    pub(crate) fn from_version_output(output: &str) -> Self {
        Self(version_allows_goals(output))
    }

    /// Whether this Codex answers the goal requests. A no on any failure — a
    /// missing binary, an unparseable version, a probe that never answered —
    /// which is Paseo's own answer when the probe throws (:7049-7052).
    pub(crate) fn probe(program: &str) -> Self {
        let memo = PROBE.get_or_init(|| Mutex::new(None));
        let Ok(mut cached) = memo.lock() else {
            return probe_once(Path::new(program));
        };
        if let Some((cached_program, goals)) = cached.as_ref() {
            if cached_program == program {
                return *goals;
            }
        }
        let goals = probe_once(Path::new(program));
        *cached = Some((PathBuf::from(program), goals));
        goals
    }
}

/// The gate, answered once per daemon process against the program that first
/// asked. Paseo memoizes the same fact on its agent factory
/// (`goalsEnabledPromise` :7012); a machine that swaps its Codex between two
/// paths re-reads here, which that memo does not.
static PROBE: OnceLock<Mutex<Option<(PathBuf, Goals)>>> = OnceLock::new();

fn probe_once(program: &Path) -> Goals {
    Goals::from_version_output(&version_output(program).unwrap_or_default())
}

/// Run `<program> --version` and answer its stdout, or the empty string when
/// the program is missing, fails, or answers later than
/// [`VERSION_PROBE_TIMEOUT`]. Paseo's `resolveBinaryVersion` answers
/// `unknown`/`error: …` for the same cases; either way no version parses and
/// the gate closes.
fn version_output(program: &Path) -> Option<String> {
    let mut command = std::process::Command::new(program);
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        // The flag `git.rs` sets for its own probes: a daemon-spawned version
        // check must not flash a console window on the desktop.
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut text = String::new();
        let mut stdout = stdout;
        let _ = stdout.read_to_string(&mut text);
        let _ = sender.send(text);
    });
    let output = match receiver.recv_timeout(VERSION_PROBE_TIMEOUT) {
        Ok(output) => output,
        // Killing the child is what lets a reader parked on a silent binary
        // exit: its pipe closes and `read_to_string` returns.
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    Some(output)
}

/// `parseCodexVersion` :173-178 + `codexVersionAtLeast` :180-191: the first
/// `major.minor.patch` run in the output, compared left to right. A version
/// that cannot be read is not at least anything — Paseo returns false.
fn version_allows_goals(output: &str) -> bool {
    let Some(version) = parse_version(output) else {
        return false;
    };
    // Slice order is lexicographic, which is exactly Paseo's left-to-right
    // `codexVersionAtLeast` loop — including the case its final `return true`
    // covers: a version equal to the minimum has goals.
    version.as_slice() >= CODEX_GOALS_MIN_VERSION.as_slice()
}

/// The first three dot-separated number runs in the output, as `0.155.1` from
/// `codex-cli 0.155.1`. Written as a scan because the workspace has no regex
/// dependency; the semantics are Paseo's `(\d+)\.(\d+)\.(\d+)` match, which
/// takes the leftmost run of digits that is followed by two more.
fn parse_version(output: &str) -> Option<[u64; 3]> {
    let bytes = output.as_bytes();
    for (start, byte) in bytes.iter().enumerate() {
        if byte.is_ascii_digit() {
            if let Some(version) = read_version_at(bytes, start) {
                return Some(version);
            }
        }
    }
    None
}

/// Read `major.minor.patch` starting at `start`, and only there — Paseo's regex
/// is anchored at the match it found, so `0.128` (no third run) is no version.
fn read_version_at(bytes: &[u8], start: usize) -> Option<[u64; 3]> {
    let mut parts = [0u64; 3];
    let mut rest = bytes.get(start..)?;
    for (position, part) in parts.iter_mut().enumerate() {
        let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        *part = std::str::from_utf8(&rest[..digits]).ok()?.parse().ok()?;
        rest = rest.get(digits..)?;
        if position < 2 {
            if rest.first() != Some(&b'.') {
                return None;
            }
            rest = rest.get(1..)?;
        }
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::{version_allows_goals, Goals};

    #[test]
    fn the_gate_reads_the_version_line_the_cli_prints() {
        // Measured on this machine: `codex --version` answers
        // `codex-cli 0.155.1`, so a prefix must not defeat the parse.
        assert!(version_allows_goals("codex-cli 0.155.1"));
        assert!(version_allows_goals("codex-cli 0.128.0"));
        assert!(!version_allows_goals("codex-cli 0.127.9"));
        assert!(!version_allows_goals("codex-cli 0.9.4"));
        assert!(version_allows_goals("1.0.0"));
        assert!(
            version_allows_goals(
                "0.128.0
extra"
            ),
            "an equal version passes"
        );
    }

    #[test]
    fn a_version_that_cannot_be_read_has_no_goals() {
        // Paseo's `parseCodexVersion` answers null and `codexVersionAtLeast`
        // answers false; `resolveBinaryVersion` can hand back `unknown` or an
        // `error: …` string, and neither may turn the flag on.
        assert!(!version_allows_goals("unknown"));
        assert!(!version_allows_goals("error: spawn failed"));
        assert!(!version_allows_goals(""));
        // Two runs are not three: `0.128` alone is not a version.
        assert!(!version_allows_goals("0.128"));
    }

    #[test]
    fn the_gate_that_failed_adds_nothing_to_the_launch_line() {
        assert_eq!(
            Goals::from_version_output("codex-cli 0.128.0").launch_args(),
            ["--enable", "goals"]
        );
        assert!(Goals::from_version_output("codex-cli 0.127.9")
            .launch_args()
            .is_empty());
    }
}
