#[cfg(any(test, unix))]
use std::collections::{BTreeMap, BTreeSet};
#[cfg(any(test, unix))]
use std::path::Path;
#[cfg(any(test, unix))]
use std::time::Instant;

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginShellCaptureState {
    NotRun,
    Applied,
    Skipped,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginShellCaptureOutcome {
    pub state: LoginShellCaptureState,
    pub applied_variables: u32,
    pub preserved_variables: u32,
}

impl LoginShellCaptureOutcome {
    fn not_run() -> Self {
        Self {
            state: LoginShellCaptureState::NotRun,
            applied_variables: 0,
            preserved_variables: 0,
        }
    }

    #[cfg(unix)]
    fn skipped() -> Self {
        Self {
            state: LoginShellCaptureState::Skipped,
            applied_variables: 0,
            preserved_variables: 0,
        }
    }

    #[cfg(unix)]
    fn failed() -> Self {
        Self {
            state: LoginShellCaptureState::Failed,
            applied_variables: 0,
            preserved_variables: 0,
        }
    }
}

static LOGIN_SHELL_CAPTURE_OUTCOME: OnceLock<LoginShellCaptureOutcome> = OnceLock::new();

pub fn login_shell_capture_outcome() -> LoginShellCaptureOutcome {
    LOGIN_SHELL_CAPTURE_OUTCOME
        .get()
        .copied()
        .unwrap_or_else(LoginShellCaptureOutcome::not_run)
}

#[cfg(unix)]
fn record_login_shell_capture_outcome(outcome: LoginShellCaptureOutcome) {
    let _ = LOGIN_SHELL_CAPTURE_OUTCOME.set(outcome);
}

#[cfg(any(test, unix))]
type EnvironmentMap = BTreeMap<Vec<u8>, Vec<u8>>;

#[cfg(any(test, unix))]
const START_MARKER: &[u8] = b"DEVBOULE_LOGIN_ENV_START";
#[cfg(any(test, unix))]
const END_MARKER: &[u8] = b"DEVBOULE_LOGIN_ENV_END";

#[cfg(any(test, unix))]
#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    MissingStartMarker,
    MissingEndMarker,
    MalformedRecord,
}

#[cfg(any(test, unix))]
#[derive(Debug, PartialEq, Eq)]
struct EnvironmentMerge {
    effective: EnvironmentMap,
    applied: EnvironmentMap,
    skipped: BTreeSet<Vec<u8>>,
}

#[cfg(any(test, unix))]
#[derive(Debug, Eq, PartialEq)]
#[cfg_attr(not(unix), allow(dead_code))]
enum ReapPoll {
    Exited,
    Running,
    Failed,
}

#[cfg(any(test, unix))]
#[derive(Debug, Eq, PartialEq)]
enum ReapOutcome {
    Exited,
    Abandoned,
}

#[cfg(any(test, unix))]
fn bounded_reap<P, N, S>(deadline: Instant, mut now: N, mut poll: P, mut sleep: S) -> ReapOutcome
where
    P: FnMut() -> ReapPoll,
    N: FnMut() -> Instant,
    S: FnMut(),
{
    loop {
        if now() >= deadline {
            return ReapOutcome::Abandoned;
        }
        match poll() {
            ReapPoll::Exited => return ReapOutcome::Exited,
            ReapPoll::Running => sleep(),
            ReapPoll::Failed => return ReapOutcome::Abandoned,
        }
    }
}

#[cfg(any(test, unix))]
/// The capture is environment-sized data, not an unbounded stream. One MiB is
/// ample for a normal process environment and noisy shell startup, while
/// preventing a broken profile from turning daemon boot into an allocation.
const CAPTURE_MAX_BYTES: u64 = 1024 * 1024;

#[cfg(any(test, unix))]
fn read_capture_file(path: &Path) -> Result<Vec<u8>, ()> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|_| ())?;
    let expected_len = file.metadata().map_err(|_| ())?.len();
    if expected_len > CAPTURE_MAX_BYTES {
        return Err(());
    }

    let mut bytes = Vec::with_capacity(expected_len as usize);
    {
        let mut limited = file.by_ref().take(CAPTURE_MAX_BYTES);
        limited.read_to_end(&mut bytes).map_err(|_| ())?;
    }
    let final_len = file.metadata().map_err(|_| ())?.len();
    if final_len != expected_len || bytes.len() as u64 != expected_len {
        return Err(());
    }
    Ok(bytes)
}

#[cfg(any(test, unix))]
fn parse_capture(raw: &[u8]) -> Result<EnvironmentMap, ParseError> {
    let mut framed = false;
    let mut saw_start = false;
    let mut parsed = EnvironmentMap::new();

    for record in raw.split(|byte| *byte == 0) {
        if !framed {
            if record == START_MARKER {
                framed = true;
                saw_start = true;
            }
            continue;
        }

        if record == END_MARKER {
            return Ok(parsed);
        }

        let Some(separator) = record.iter().position(|byte| *byte == b'=') else {
            return Err(ParseError::MalformedRecord);
        };
        if separator == 0 {
            return Err(ParseError::MalformedRecord);
        }
        // `env -0` cannot emit NUL bytes inside a record, so a marker that
        // occurs inside a value remains part of that value rather than
        // becoming a false frame boundary.
        parsed.insert(
            record[..separator].to_vec(),
            record[separator + 1..].to_vec(),
        );
    }

    if !saw_start {
        Err(ParseError::MissingStartMarker)
    } else {
        Err(ParseError::MissingEndMarker)
    }
}

#[cfg(any(test, unix))]
/// Preserve variables that describe the process context supplied by the GUI
/// launcher, while importing variables that describe the user's preferences
/// from the login shell. `DEVBOULE_*` is daemon-owned; the named display,
/// runtime, IPC, and temporary-directory variables are launcher/session-owned.
/// `SSH_AUTH_SOCK` is preserved as session context: replacing it with a shell's
/// unrelated agent socket could make GUI-launched authentication use the wrong
/// agent. Add a new key here by applying that meaning-based rule, not by
/// guessing from whether the shell happened to export it.
fn is_preserved_key(key: &[u8]) -> bool {
    key.starts_with(b"DEVBOULE_")
        || matches!(
            key,
            b"TMPDIR"
                | b"XPC_SERVICE_NAME"
                | b"XPC_FLAGS"
                | b"__CF_USER_TEXT_ENCODING"
                | b"XDG_RUNTIME_DIR"
                | b"DBUS_SESSION_BUS_ADDRESS"
                | b"DISPLAY"
                | b"WAYLAND_DISPLAY"
                | b"SSH_AUTH_SOCK"
        )
}

#[cfg(any(test, unix))]
fn capture_is_usable(captured: &EnvironmentMap) -> bool {
    captured
        .get(b"PATH" as &[u8])
        .is_some_and(|path| !path.is_empty())
}

#[cfg(any(test, unix))]
fn merge_environment(current: &EnvironmentMap, captured: &EnvironmentMap) -> EnvironmentMerge {
    let mut effective = current.clone();
    let mut applied = EnvironmentMap::new();
    let mut skipped = BTreeSet::new();

    for (key, value) in captured {
        if is_preserved_key(key) {
            skipped.insert(key.clone());
            continue;
        }
        effective.insert(key.clone(), value.clone());
        applied.insert(key.clone(), value.clone());
    }

    EnvironmentMerge {
        effective,
        applied,
        skipped,
    }
}

#[cfg(unix)]
mod unix_execution {
    use std::ffi::OsString;
    use std::fs::{self, File, OpenOptions};
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use super::{
        merge_environment, parse_capture, record_login_shell_capture_outcome, EnvironmentMap,
        LoginShellCaptureOutcome, LoginShellCaptureState,
    };

    const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(5);
    const CHILD_REAP_TIMEOUT: Duration = Duration::from_secs(1);
    const WAIT_POLL: Duration = Duration::from_millis(10);
    const CAPTURE_SCRIPT: &str = concat!(
        "set -e\n",
        "printf '\\000%s\\000' 'DEVBOULE_LOGIN_ENV_START'\n",
        "env -0\n",
        "printf '%s\\000' 'DEVBOULE_LOGIN_ENV_END'\n",
    );

    pub(super) fn initialize_login_shell_environment() {
        let captured = match capture_login_environment() {
            Ok(captured) => captured,
            Err(()) => {
                record_login_shell_capture_outcome(LoginShellCaptureOutcome::failed());
                return;
            }
        };
        if captured.is_empty() || !super::capture_is_usable(&captured) {
            record_login_shell_capture_outcome(LoginShellCaptureOutcome::skipped());
            return;
        }

        let current = current_environment();
        let merged = merge_environment(&current, &captured);
        let outcome = LoginShellCaptureOutcome {
            state: LoginShellCaptureState::Applied,
            applied_variables: u32::try_from(merged.applied.len()).unwrap_or(u32::MAX),
            preserved_variables: u32::try_from(merged.skipped.len()).unwrap_or(u32::MAX),
        };
        for (key, value) in merged.applied {
            std::env::set_var(OsString::from_vec(key), OsString::from_vec(value));
        }
        record_login_shell_capture_outcome(outcome);
    }

    fn capture_login_environment() -> Result<EnvironmentMap, ()> {
        let shell = std::env::var_os("SHELL")
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| OsString::from("/bin/sh"));
        let (path, output_file) = capture_file().ok_or(())?;
        let mut child = match Command::new(shell)
            .args(["-ilc", CAPTURE_SCRIPT])
            .stdin(Stdio::null())
            .stdout(Stdio::from(output_file))
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => {
                remove_capture_file(&path);
                return Err(());
            }
        };

        let deadline = Instant::now() + LOGIN_SHELL_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    reap_after_kill(&mut child);
                    remove_capture_file(&path);
                    return Err(());
                }
                Ok(None) => thread::sleep(WAIT_POLL),
                Err(_) => {
                    let _ = child.kill();
                    reap_after_kill(&mut child);
                    remove_capture_file(&path);
                    return Err(());
                }
            }
        };

        let bytes = super::read_capture_file(&path).ok();
        remove_capture_file(&path);
        if !status.success() {
            return Err(());
        }
        parse_capture(&bytes.ok_or(())?).map_err(|_| ())
    }

    fn reap_after_kill(child: &mut std::process::Child) {
        let deadline = Instant::now() + CHILD_REAP_TIMEOUT;
        // Never use an unbounded wait after kill: a process stuck in an
        // uninterruptible kernel state may never be reapable. Abandoning the
        // child can leave a zombie, but that is safer than blocking daemon boot.
        let _ = super::bounded_reap(
            deadline,
            Instant::now,
            || match child.try_wait() {
                Ok(Some(_)) => super::ReapPoll::Exited,
                Ok(None) => super::ReapPoll::Running,
                Err(_) => super::ReapPoll::Failed,
            },
            || thread::sleep(WAIT_POLL),
        );
    }

    fn capture_file() -> Option<(PathBuf, File)> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = std::env::temp_dir();
        for attempt in 0..8 {
            let path = directory.join(format!(
                ".devboule-login-env-{}-{stamp}-{attempt}.tmp",
                std::process::id()
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true).mode(0o600);
            match options.open(&path) {
                Ok(file) => return Some((path, file)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return None,
            }
        }
        None
    }

    fn remove_capture_file(path: &std::path::Path) {
        let _ = fs::remove_file(path);
    }

    fn current_environment() -> EnvironmentMap {
        std::env::vars_os()
            .map(|(key, value)| (key.into_vec(), value.into_vec()))
            .collect()
    }
}

#[cfg(unix)]
pub fn initialize_login_shell_environment() {
    unix_execution::initialize_login_shell_environment();
}

#[cfg(not(unix))]
pub fn initialize_login_shell_environment() {}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &[u8] = b"DEVBOULE_LOGIN_ENV_START";
    const END: &[u8] = b"DEVBOULE_LOGIN_ENV_END";

    fn framed(records: &[&[u8]]) -> Vec<u8> {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"banner before\n");
        raw.push(0);
        raw.extend_from_slice(START);
        raw.push(0);
        for record in records {
            raw.extend_from_slice(record);
            raw.push(0);
        }
        raw.extend_from_slice(END);
        raw.push(0);
        raw.extend_from_slice(b"fortune after\n");
        raw
    }

    #[test]
    fn parser_keeps_only_framed_nul_records_and_preserves_raw_values() {
        let raw = framed(&[
            b"PATH=/usr/local/bin=/with=equals",
            b"MULTILINE=first line\nsecond line",
            b"EMPTY=",
            b"MARKER=before-DEVBOULE_LOGIN_ENV_END-after",
            b"BINARY=\xff\xfe",
        ]);

        let parsed = parse_capture(&raw).expect("capture should parse");

        assert_eq!(
            parsed.get(b"PATH" as &[u8]),
            Some(&b"/usr/local/bin=/with=equals".to_vec())
        );
        assert_eq!(
            parsed.get(b"MULTILINE" as &[u8]),
            Some(&b"first line\nsecond line".to_vec())
        );
        assert_eq!(parsed.get(b"EMPTY" as &[u8]), Some(&Vec::new()));
        assert_eq!(
            parsed.get(b"MARKER" as &[u8]),
            Some(&b"before-DEVBOULE_LOGIN_ENV_END-after".to_vec())
        );
        assert_eq!(parsed.get(b"BINARY" as &[u8]), Some(&vec![0xff, 0xfe]));
        assert!(!parsed.contains_key(b"banner before\n" as &[u8]));
    }

    #[test]
    fn parser_uses_the_last_duplicate_key() {
        let parsed = parse_capture(&framed(&[b"DUPLICATE=first", b"DUPLICATE=last"]))
            .expect("duplicate keys should parse");

        assert_eq!(parsed.get(b"DUPLICATE" as &[u8]), Some(&b"last".to_vec()));
    }

    #[test]
    fn parser_rejects_missing_markers_and_truncated_records() {
        assert!(parse_capture(b"PATH=/bin\0DEVBOULE_LOGIN_ENV_END\0").is_err());
        assert!(parse_capture(b"DEVBOULE_LOGIN_ENV_START\0PATH=/bin\0").is_err());
        assert!(parse_capture(&framed(&[b"GOOD=value", b"BROKEN"])).is_err());

        let mut truncated = Vec::from(START);
        truncated.extend_from_slice(b"\0PATH=/bin\0BROKEN=partial");
        assert!(parse_capture(&truncated).is_err());
    }

    #[test]
    fn merge_preserves_daemon_controls_and_applies_login_values() {
        let current = BTreeMap::from([
            (b"PATH".to_vec(), b"/usr/bin".to_vec()),
            (b"TMPDIR".to_vec(), b"/var/folders/gui/T/".to_vec()),
            (
                b"XPC_SERVICE_NAME".to_vec(),
                b"com.example.devboule".to_vec(),
            ),
            (b"XPC_FLAGS".to_vec(), b"0x0".to_vec()),
            (b"__CF_USER_TEXT_ENCODING".to_vec(), b"0x1F5:0:0".to_vec()),
            (b"XDG_RUNTIME_DIR".to_vec(), b"/run/user/1000".to_vec()),
            (
                b"DBUS_SESSION_BUS_ADDRESS".to_vec(),
                b"unix:path=/run/user/1000/bus".to_vec(),
            ),
            (b"DISPLAY".to_vec(), b":0".to_vec()),
            (b"WAYLAND_DISPLAY".to_vec(), b"wayland-0".to_vec()),
            (
                b"SSH_AUTH_SOCK".to_vec(),
                b"/private/tmp/agent.sock".to_vec(),
            ),
            (
                b"DEVBOULE_RUNTIME_DIR".to_vec(),
                b"/private/runtime".to_vec(),
            ),
            (b"DEVBOULE_SHELL".to_vec(), b"/bin/zsh".to_vec()),
            (b"UNCHANGED".to_vec(), b"current".to_vec()),
        ]);
        let captured = BTreeMap::from([
            (b"PATH".to_vec(), b"/opt/bin:/usr/bin".to_vec()),
            (b"TMPDIR".to_vec(), b"/tmp/login-shell".to_vec()),
            (b"XPC_SERVICE_NAME".to_vec(), b"wrong.service".to_vec()),
            (b"XPC_FLAGS".to_vec(), b"0x1".to_vec()),
            (b"__CF_USER_TEXT_ENCODING".to_vec(), b"0x0:0:0".to_vec()),
            (b"XDG_RUNTIME_DIR".to_vec(), b"/tmp/runtime".to_vec()),
            (
                b"DBUS_SESSION_BUS_ADDRESS".to_vec(),
                b"unix:path=/tmp/bus".to_vec(),
            ),
            (b"DISPLAY".to_vec(), b"".to_vec()),
            (b"WAYLAND_DISPLAY".to_vec(), b"".to_vec()),
            (b"SSH_AUTH_SOCK".to_vec(), b"/tmp/login-agent.sock".to_vec()),
            (b"DEVBOULE_RUNTIME_DIR".to_vec(), b"/wrong/runtime".to_vec()),
            (b"DEVBOULE_SHELL".to_vec(), b"/wrong/shell".to_vec()),
            (b"NEW_VALUE".to_vec(), b"from-login-shell".to_vec()),
        ]);

        let merged = merge_environment(&current, &captured);

        assert_eq!(
            merged.applied.get(b"PATH" as &[u8]),
            Some(&b"/opt/bin:/usr/bin".to_vec())
        );
        assert_eq!(
            merged.applied.get(b"NEW_VALUE" as &[u8]),
            Some(&b"from-login-shell".to_vec())
        );
        assert_eq!(
            merged.skipped,
            BTreeSet::from([
                b"DBUS_SESSION_BUS_ADDRESS".to_vec(),
                b"DEVBOULE_RUNTIME_DIR".to_vec(),
                b"DEVBOULE_SHELL".to_vec(),
                b"DISPLAY".to_vec(),
                b"SSH_AUTH_SOCK".to_vec(),
                b"TMPDIR".to_vec(),
                b"WAYLAND_DISPLAY".to_vec(),
                b"XDG_RUNTIME_DIR".to_vec(),
                b"XPC_FLAGS".to_vec(),
                b"XPC_SERVICE_NAME".to_vec(),
                b"__CF_USER_TEXT_ENCODING".to_vec(),
            ])
        );
        assert_eq!(
            merged.effective.get(b"DEVBOULE_RUNTIME_DIR" as &[u8]),
            Some(&b"/private/runtime".to_vec())
        );
        assert_eq!(
            merged.effective.get(b"DEVBOULE_SHELL" as &[u8]),
            Some(&b"/bin/zsh".to_vec())
        );
        for key in [
            b"TMPDIR" as &[u8],
            b"XPC_SERVICE_NAME",
            b"XPC_FLAGS",
            b"__CF_USER_TEXT_ENCODING",
            b"XDG_RUNTIME_DIR",
            b"DBUS_SESSION_BUS_ADDRESS",
            b"DISPLAY",
            b"WAYLAND_DISPLAY",
            b"SSH_AUTH_SOCK",
        ] {
            assert_eq!(merged.effective.get(key), current.get(key), "{key:?}");
            assert!(!merged.applied.contains_key(key), "{key:?}");
            assert!(merged.skipped.contains(key), "{key:?}");
        }
    }

    #[test]
    fn unusable_capture_is_not_applied() {
        assert!(!capture_is_usable(&EnvironmentMap::new()));
        assert!(!capture_is_usable(&BTreeMap::from([(
            b"PATH".to_vec(),
            Vec::new(),
        )])));
        assert!(capture_is_usable(&BTreeMap::from([(
            b"PATH".to_vec(),
            b"/usr/bin".to_vec(),
        )])));
    }

    #[test]
    fn bounded_reap_abandons_a_child_that_never_exits() {
        let start = std::time::Instant::now();
        let now = std::cell::Cell::new(start);
        let outcome = bounded_reap(
            start + std::time::Duration::from_secs(3),
            || now.get(),
            || ReapPoll::Running,
            || now.set(now.get() + std::time::Duration::from_secs(1)),
        );

        assert_eq!(outcome, ReapOutcome::Abandoned);
    }

    #[test]
    fn capture_read_rejects_a_file_over_the_boot_cap() {
        let path = std::env::temp_dir().join(format!(
            ".devboule-login-env-test-{}-{}.tmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::write(&path, vec![b'x'; (CAPTURE_MAX_BYTES + 1) as usize])
            .expect("oversized capture");

        assert!(read_capture_file(&path).is_err());
        let _ = std::fs::remove_file(path);
    }
}
