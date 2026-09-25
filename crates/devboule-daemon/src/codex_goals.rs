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
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// `CODEX_GOALS_MIN_VERSION` :170. Below it Codex rejects `--enable goals` at
/// launch, so the flag must not be passed at all.
const CODEX_GOALS_MIN_VERSION: [u64; 3] = [0, 128, 0];

/// How long one `--version` probe may take. Paseo's is 5 s
/// (`diagnostic-utils.ts:138-147`), and a probe that outlives it reads as a
/// binary without the feature.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Memoized gate answers, keyed by the probed argv: the resolved program
/// plus the prefix args the probe ran with, so two shims sharing one
/// `node.exe` never share an answer.
type ProbedVersions = std::collections::HashMap<(PathBuf, Vec<String>), Goals>;

static PROBED_VERSIONS: OnceLock<Mutex<ProbedVersions>> = OnceLock::new();

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
    ///
    /// `launch_args` is the argv the session will spawn with, `app-server`
    /// included: where our launch differs from Paseo's, the probe must measure
    /// Codex, so it runs that same argv with `--version` in place of
    /// `app-server`. Probing the program alone measures `node` on npm-shim
    /// installs (`node.exe <script> app-server`), and node's version always
    /// passes the gate — Paseo never meets that shape because its command is
    /// always the `codex` binary (`resolveCodexLaunchPrefix`).
    pub(crate) fn probe(program: &str, launch_args: &[String]) -> Self {
        let (program, probe_argv) = probe_invocation(program, launch_args);
        let key = (program.clone(), probe_argv.clone());
        if let Some(result) = PROBED_VERSIONS
            .get_or_init(Default::default)
            .lock()
            .ok()
            .and_then(|cache| cache.get(&key).copied())
        {
            return result;
        }
        let result = probe_once(&program, &probe_argv);
        // Only successes memoize. A failed probe — a missing binary, a slow
        // disk that outlived the timeout, a Codex upgraded mid-life — is
        // re-run on the next create instead of closing the gate for the
        // daemon's life, at the price of a fresh bounded child (up to
        // `VERSION_PROBE_TIMEOUT`) on the creating thread for every create
        // until one passes. Paseo pays a probe per agent instance (:7031);
        // the success memo keeps that cost off every create once a binary
        // answers.
        if result.enabled() {
            if let Ok(mut cache) = PROBED_VERSIONS.get_or_init(Default::default).lock() {
                cache.insert(key, result);
            }
        }
        result
    }
}

fn probe_once(program: &Path, probe_argv: &[String]) -> Goals {
    Goals::from_version_output(&version_output(program, probe_argv).unwrap_or_default())
}

/// The probe invocation for a launch: the resolved program plus the launch
/// argv with `app-server` traded for `--version`. Pure — no child is
/// spawned — so tests pin the shim shape without running anything, and
/// `Goals::probe` runs exactly this invocation, so those tests guard the
/// production call site (`session_goals`) as well as the helper.
pub(crate) fn probe_invocation(program: &str, launch_args: &[String]) -> (PathBuf, Vec<String>) {
    (
        resolved_program_path(program),
        version_probe_argv(launch_args),
    )
}

fn resolved_program_path(program: &str) -> PathBuf {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| std::fs::canonicalize(&candidate).ok())
        .unwrap_or_else(|| path.to_path_buf())
}

/// Run the probe and answer its stdout, or the empty string when the program
/// is missing, fails, or answers later than [`VERSION_PROBE_TIMEOUT`].
/// Paseo's `resolveBinaryVersion` answers `unknown`/`error: …` for the same
/// cases; either way no version parses and the gate closes.
///
/// `probe_argv` is already the complete argv — [`version_probe_argv`] built
/// it — so nothing is appended here. A second `--version` would read as a
/// second positional to an npm script, which is the exact-flag-twice trap:
/// the version-printing-binary test fails if one is ever added.
fn version_output(program: &Path, probe_argv: &[String]) -> Option<String> {
    let started = Instant::now();
    let mut command = std::process::Command::new(program);
    command
        .args(probe_argv)
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
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if started.elapsed() >= VERSION_PROBE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let _ = reader.join();
    status.success().then_some(output)
}

/// The argv the launch runs, with `--version` in place of `app-server`:
/// fix 1's shape, kept because only it measures Codex on shim installs.
/// A trailing `app-server` is the launch's own flag, so it is dropped; any
/// other trailing argument is kept, and `--version` goes last either way.
fn version_probe_argv(launch_args: &[String]) -> Vec<String> {
    let mut argv: Vec<String> = match launch_args.split_last() {
        Some((last, rest)) if last == "app-server" => rest.to_vec(),
        _ => launch_args.to_vec(),
    };
    argv.push("--version".to_string());
    argv
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
    use super::{
        probe_invocation, resolved_program_path, version_allows_goals, version_probe_argv, Goals,
        PROBED_VERSIONS,
    };

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

    #[test]
    fn the_probe_invocation_keeps_a_shim_launch_whole() {
        // The call-site decision fix2 broke for lack of a pin: a shim-shaped
        // launch (`node.exe` + a Codex script + `app-server`) must probe as
        // `node.exe <script> --version`. This runs the same pure invocation
        // `Goals::probe` — and therefore the production `session_goals` line
        // — runs, so neutering the launch args here (the `&[]` regression)
        // fails this test without spawning anything.
        let launch = |script: &str| {
            probe_invocation(
                "C:/shim/node.exe",
                &[script.to_string(), "app-server".to_string()],
            )
        };
        assert_eq!(
            launch("C:/shim/codex.js"),
            (
                std::path::PathBuf::from("C:/shim/node.exe"),
                vec!["C:/shim/codex.js".to_string(), "--version".to_string()],
            )
        );
        assert_ne!(
            launch("C:/shim/codex.js").1,
            launch("C:/other/codex.js").1,
            "two shims sharing one node.exe build different invocations, so they never share a memo answer"
        );
        assert_eq!(
            probe_invocation("codex", &[]),
            (
                resolved_program_path("codex"),
                vec!["--version".to_string()],
            ),
            "a native launch probes the program alone"
        );
    }

    #[test]
    fn the_probe_argv_trades_app_server_for_version() {
        // The shim shape fix 1 measured: `node.exe <script> app-server`
        // probes as `node.exe <script> --version`, never as `node --version`.
        assert_eq!(
            version_probe_argv(&["C:/node/codex.js".to_string(), "app-server".to_string()]),
            ["C:/node/codex.js", "--version"],
        );
        assert_eq!(version_probe_argv(&[]), ["--version"]);
        assert_eq!(
            version_probe_argv(&["codex".to_string(), "--flag".to_string()]),
            ["codex", "--flag", "--version"],
            "only a trailing app-server is the launch flag"
        );
    }

    #[test]
    fn a_missing_binary_fails_closed_and_is_not_memoized() {
        // A failed probe must be retried on the next create, not close the
        // gate for the daemon's life: failures leave no memo behind.
        let missing = std::env::temp_dir().join("devboule-codex-probe-does-not-exist");
        let missing = missing.to_string_lossy().into_owned();
        assert!(!Goals::probe(&missing, &[]).enabled());
        assert!(!Goals::probe(&missing, &["app-server".to_string()]).enabled());
        let cache = PROBED_VERSIONS.get_or_init(Default::default);
        let cache = cache.lock().expect("the probe memo is readable");
        assert!(
            cache
                .keys()
                .all(|(program, _)| program.to_string_lossy() != missing),
            "a failed probe memoizes nothing"
        );
    }

    #[test]
    fn a_version_printing_binary_passes_and_is_memoized() {
        // The green path: a real `--version` child whose stdout parses.
        // `rustc` is the version printer only when it is runnable here — a
        // machine that runs `cargo` by absolute path with the toolchain off
        // PATH skips instead of failing on an environment it never promised.
        if let Some(reason) = crate::test_support::external_program_skip_reason("rustc") {
            eprintln!("{reason}");
            return;
        }
        // `rustc 1.x.y` parses above the 0.128.0 minimum. If a second
        // `--version` is ever appended at the spawn site, rustc refuses the
        // doubled flag and this fails — that is the A1 trap, pinned here
        // rather than on the argv helper alone.
        assert!(Goals::probe("rustc", &[]).enabled());
        let key = (
            resolved_program_path("rustc"),
            vec!["--version".to_string()],
        );
        let cache = PROBED_VERSIONS.get_or_init(Default::default);
        let cache = cache.lock().expect("the probe memo is readable");
        assert!(
            cache.get(&key).is_some_and(|goals| goals.enabled()),
            "the passed probe is memoized under its own program and argv, not just anywhere"
        );
    }
}
