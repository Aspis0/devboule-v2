use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

// Local cold measurements of `git -C <path> rev-parse --show-toplevel` are
// below 150 ms, but the Windows launcher, first-start antivirus scans, and
// network-backed folders need a much larger operational margin. Ten seconds
// keeps a hung probe bounded while making ordinary cold-start slowness a
// retryable `TimedOut` result instead of a misleading `NotRepository` result.
const GIT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const GIT_REAP_TIMEOUT: Duration = Duration::from_millis(500);
const GIT_PROBE_POLL: Duration = Duration::from_millis(10);
const GIT_OUTPUT_READ_TIMEOUT: Duration = Duration::from_millis(250);
const GIT_STDOUT_MAX_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitRepositoryStatus {
    RepositoryRoot,
    InsideRepository,
    NotRepository,
    TimedOut,
    Unknown,
}

impl GitRepositoryStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RepositoryRoot => "repository",
            Self::InsideRepository => "inside_repository",
            Self::NotRepository => "not_repository",
            Self::TimedOut => "timed_out",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GitReapPoll {
    Exited,
    Running,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GitReapOutcome {
    Exited,
    TimedOut,
    Failed,
}

fn bounded_reap<P, N, S>(deadline: Instant, mut now: N, mut poll: P, mut sleep: S) -> GitReapOutcome
where
    P: FnMut() -> GitReapPoll,
    N: FnMut() -> Instant,
    S: FnMut(),
{
    loop {
        match poll() {
            GitReapPoll::Exited => return GitReapOutcome::Exited,
            GitReapPoll::Failed => return GitReapOutcome::Failed,
            GitReapPoll::Running => {}
        }
        if now() >= deadline {
            return GitReapOutcome::TimedOut;
        }
        sleep();
    }
}

/// Ask git for the repository root without invoking a shell. A directory
/// below a repository is deliberately not classified as the repository root.
pub(crate) fn detect_git_repository(path: &Path) -> GitRepositoryStatus {
    detect_git_repository_with_program(path, OsStr::new("git"), &[])
}

fn detect_git_repository_with_program(
    path: &Path,
    program: &OsStr,
    prefix_args: &[OsString],
) -> GitRepositoryStatus {
    let mut command = Command::new(program);
    command
        .args(prefix_args)
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"]);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW prevents a desktop app from flashing a console for
        // every Git probe made while registering or refreshing a project.
        command.creation_flags(0x0800_0000);
    }

    let mut process = match spawn_git_process(command) {
        Ok(process) => process,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return GitRepositoryStatus::Unknown
        }
        Err(_) => return GitRepositoryStatus::Unknown,
    };
    let mut exit_status = None;
    let outcome = bounded_reap(
        Instant::now() + GIT_PROBE_TIMEOUT,
        Instant::now,
        || match process.child.try_wait() {
            Ok(Some(status)) => {
                exit_status = Some(status);
                GitReapPoll::Exited
            }
            Ok(None) => GitReapPoll::Running,
            Err(_) => GitReapPoll::Failed,
        },
        || thread::sleep(GIT_PROBE_POLL),
    );
    match outcome {
        GitReapOutcome::TimedOut => {
            terminate_git_process(&mut process);
            GitRepositoryStatus::TimedOut
        }
        GitReapOutcome::Failed => {
            terminate_git_process(&mut process);
            GitRepositoryStatus::Unknown
        }
        GitReapOutcome::Exited => {
            let Some(status) = exit_status else {
                terminate_git_process(&mut process);
                return GitRepositoryStatus::Unknown;
            };
            let Some(bytes) = receive_git_stdout(&mut process) else {
                terminate_git_process(&mut process);
                return GitRepositoryStatus::TimedOut;
            };
            if !status.success() {
                return GitRepositoryStatus::NotRepository;
            }
            let Some(root) = parse_git_root(&bytes) else {
                return GitRepositoryStatus::Unknown;
            };
            let Some(root) = canonicalize_for_detection(&root) else {
                return GitRepositoryStatus::Unknown;
            };
            let Some(path) = canonicalize_for_detection(path) else {
                return GitRepositoryStatus::Unknown;
            };
            classify_git_probe(&path, Some(&root), true)
        }
    }
}

struct GitProcess {
    child: Child,
    stdout: Receiver<Option<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
    #[cfg(windows)]
    job: crate::process_tree::JobObject,
}

fn append_git_stdout(output: &mut Vec<u8>, chunk: &[u8]) {
    let remaining = (GIT_STDOUT_MAX_BYTES + 1).saturating_sub(output.len());
    output.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
}

fn spawn_git_process(mut command: Command) -> std::io::Result<GitProcess> {
    #[cfg(windows)]
    let job = crate::process_tree::JobObject::new()?;
    let mut child = command.spawn()?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        if let Err(error) = job.assign(child.as_raw_handle()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    }
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            #[cfg(windows)]
            let _ = job.terminate_and_wait(GIT_REAP_TIMEOUT);
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::other("git probe stdout was not piped"));
        }
    };
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let mut stdout = stdout;
        let mut bytes = Vec::with_capacity(GIT_STDOUT_MAX_BYTES + 1);
        let mut buffer = [0u8; 4096];
        let mut read_error = false;
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) => break,
                Ok(length) => {
                    append_git_stdout(&mut bytes, &buffer[..length]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    read_error = true;
                    break;
                }
            }
        }
        let result = (!read_error).then_some(bytes);
        let _ = sender.send(result);
    });
    Ok(GitProcess {
        child,
        stdout: receiver,
        reader: Some(reader),
        #[cfg(windows)]
        job,
    })
}

fn receive_git_stdout(process: &mut GitProcess) -> Option<Vec<u8>> {
    let result = process.stdout.recv_timeout(GIT_OUTPUT_READ_TIMEOUT).ok()?;
    if let Some(reader) = process.reader.take() {
        let _ = reader.join();
    }
    result
}

fn terminate_git_process(process: &mut GitProcess) {
    #[cfg(windows)]
    let _ = process.job.terminate_and_wait(GIT_REAP_TIMEOUT);
    let _ = process.child.kill();
    let _ = bounded_reap(
        Instant::now() + GIT_REAP_TIMEOUT,
        Instant::now,
        || match process.child.try_wait() {
            Ok(Some(_)) => GitReapPoll::Exited,
            Ok(None) => GitReapPoll::Running,
            Err(_) => GitReapPoll::Failed,
        },
        || thread::sleep(GIT_PROBE_POLL),
    );
    if process.stdout.recv_timeout(GIT_REAP_TIMEOUT).is_ok() {
        if let Some(reader) = process.reader.take() {
            let _ = reader.join();
        }
    }
}

fn parse_git_root(stdout: &[u8]) -> Option<PathBuf> {
    if stdout.len() > GIT_STDOUT_MAX_BYTES {
        return None;
    }
    let text = std::str::from_utf8(stdout).ok()?;
    let root = text.trim_end_matches(['\r', '\n']);
    (!root.is_empty()).then(|| PathBuf::from(root))
}

fn canonicalize_for_detection(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

fn same_path(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .replace('/', "\\")
            .eq_ignore_ascii_case(&right.to_string_lossy().replace('/', "\\"))
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

pub(crate) fn classify_git_probe(
    path: &Path,
    repository_root: Option<&Path>,
    git_available: bool,
) -> GitRepositoryStatus {
    if !git_available {
        return GitRepositoryStatus::Unknown;
    }
    match repository_root {
        Some(root) if same_path(path, root) => GitRepositoryStatus::RepositoryRoot,
        Some(_) => GitRepositoryStatus::InsideRepository,
        None => GitRepositoryStatus::NotRepository,
    }
}

const GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl GitOutput {
    pub(crate) fn error_message(&self, program: &str) -> String {
        let stderr = self.stderr.trim();
        let stdout = self.stdout.trim();
        if !stderr.is_empty() {
            stderr.to_string()
        } else if !stdout.is_empty() {
            stdout.to_string()
        } else {
            format!("{program} failed with status {:?}", self.code)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GitRunError {
    NotFound,
    TimedOut,
    SpawnFailed,
}

/// Run `git` with the given argv. Stdout and stderr are drained concurrently
/// under a Job Object so a Windows launcher cannot outlive a timeout, and a
/// full pipe cannot deadlock the wait.
pub(crate) fn run_git_args(args: &[String]) -> Result<GitOutput, GitRunError> {
    let mut command = Command::new("git");
    command.args(args);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }

    let mut process = match spawn_captured_git_process(command) {
        Ok(process) => process,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(GitRunError::NotFound)
        }
        Err(_) => return Err(GitRunError::SpawnFailed),
    };
    let mut exit_status = None;
    let outcome = bounded_reap(
        Instant::now() + GIT_COMMAND_TIMEOUT,
        Instant::now,
        || match process.child.try_wait() {
            Ok(Some(status)) => {
                exit_status = Some(status);
                GitReapPoll::Exited
            }
            Ok(None) => GitReapPoll::Running,
            Err(_) => GitReapPoll::Failed,
        },
        || thread::sleep(GIT_PROBE_POLL),
    );
    match outcome {
        GitReapOutcome::TimedOut | GitReapOutcome::Failed => {
            terminate_captured_git_process(&mut process);
            Err(GitRunError::TimedOut)
        }
        GitReapOutcome::Exited => {
            let Some(status) = exit_status else {
                terminate_captured_git_process(&mut process);
                return Err(GitRunError::TimedOut);
            };
            // The process has already exited. A post-checkout hook that
            // inherited the pipes can delay EOF past GIT_OUTPUT_READ_TIMEOUT;
            // that must not turn a completed git into TimedOut.
            let (stdout, stderr) = receive_captured_git_stdio(&mut process)
                .unwrap_or_else(|| take_available_git_stdio(&mut process));
            Ok(GitOutput {
                success: status.success(),
                code: status.code(),
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            })
        }
    }
}

struct CapturedGitProcess {
    child: Child,
    stdout: Receiver<Option<Vec<u8>>>,
    stderr: Receiver<Option<Vec<u8>>>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<()>>,
    #[cfg(windows)]
    job: crate::process_tree::JobObject,
}

fn spawn_stdio_reader(
    mut stream: impl Read + Send + 'static,
) -> (Receiver<Option<Vec<u8>>>, JoinHandle<()>) {
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let mut bytes = Vec::with_capacity(GIT_STDOUT_MAX_BYTES + 1);
        let mut buffer = [0u8; 4096];
        let mut read_error = false;
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(length) => append_git_stdout(&mut bytes, &buffer[..length]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    read_error = true;
                    break;
                }
            }
        }
        let _ = sender.send((!read_error).then_some(bytes));
    });
    (receiver, reader)
}

fn spawn_captured_git_process(mut command: Command) -> std::io::Result<CapturedGitProcess> {
    #[cfg(windows)]
    let job = crate::process_tree::JobObject::new()?;
    let mut child = command.spawn()?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        if let Err(error) = job.assign(child.as_raw_handle()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    }
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            #[cfg(windows)]
            let _ = job.terminate_and_wait(GIT_REAP_TIMEOUT);
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::other("git stdout was not piped"));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            #[cfg(windows)]
            let _ = job.terminate_and_wait(GIT_REAP_TIMEOUT);
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::other("git stderr was not piped"));
        }
    };
    let (stdout_rx, stdout_reader) = spawn_stdio_reader(stdout);
    let (stderr_rx, stderr_reader) = spawn_stdio_reader(stderr);
    Ok(CapturedGitProcess {
        child,
        stdout: stdout_rx,
        stderr: stderr_rx,
        stdout_reader: Some(stdout_reader),
        stderr_reader: Some(stderr_reader),
        #[cfg(windows)]
        job,
    })
}

fn receive_captured_git_stdio(process: &mut CapturedGitProcess) -> Option<(Vec<u8>, Vec<u8>)> {
    let stdout = process.stdout.recv_timeout(GIT_OUTPUT_READ_TIMEOUT).ok()?;
    let stderr = process.stderr.recv_timeout(GIT_OUTPUT_READ_TIMEOUT).ok()?;
    if let Some(reader) = process.stdout_reader.take() {
        let _ = reader.join();
    }
    if let Some(reader) = process.stderr_reader.take() {
        let _ = reader.join();
    }
    Some((stdout?, stderr?))
}

fn take_available_git_stdio(process: &mut CapturedGitProcess) -> (Vec<u8>, Vec<u8>) {
    let stdout = match process.stdout.try_recv() {
        Ok(Some(bytes)) => bytes,
        _ => Vec::new(),
    };
    let stderr = match process.stderr.try_recv() {
        Ok(Some(bytes)) => bytes,
        _ => Vec::new(),
    };
    (stdout, stderr)
}

fn terminate_captured_git_process(process: &mut CapturedGitProcess) {
    #[cfg(windows)]
    let _ = process.job.terminate_and_wait(GIT_REAP_TIMEOUT);
    let _ = process.child.kill();
    let _ = bounded_reap(
        Instant::now() + GIT_REAP_TIMEOUT,
        Instant::now,
        || match process.child.try_wait() {
            Ok(Some(_)) => GitReapPoll::Exited,
            Ok(None) => GitReapPoll::Running,
            Err(_) => GitReapPoll::Failed,
        },
        || thread::sleep(GIT_PROBE_POLL),
    );
    if process.stdout.recv_timeout(GIT_REAP_TIMEOUT).is_ok() {
        if let Some(reader) = process.stdout_reader.take() {
            let _ = reader.join();
        }
    }
    if process.stderr.recv_timeout(GIT_REAP_TIMEOUT).is_ok() {
        if let Some(reader) = process.stderr_reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        append_git_stdout, bounded_reap, classify_git_probe, detect_git_repository,
        detect_git_repository_with_program, parse_git_root, GitReapOutcome, GitReapPoll,
        GitRepositoryStatus, GIT_STDOUT_MAX_BYTES,
    };

    #[test]
    fn a_nested_folder_is_not_reported_as_the_repository_root() {
        let repo = Path::new(r"C:\repositories\project");
        let nested = repo.join("src");
        assert_eq!(
            classify_git_probe(&nested, Some(repo), true),
            GitRepositoryStatus::InsideRepository
        );
    }

    #[test]
    fn missing_git_is_distinct_from_a_folder_with_no_repository() {
        let path = Path::new(r"C:\Users\alice\project");
        assert_eq!(
            classify_git_probe(path, None, false),
            GitRepositoryStatus::Unknown
        );
        assert_eq!(
            classify_git_probe(path, None, true),
            GitRepositoryStatus::NotRepository
        );
    }

    #[test]
    fn bounded_git_reap_reports_timeout_without_waiting_forever() {
        let start = std::time::Instant::now();
        let now = std::cell::Cell::new(start);
        let outcome = bounded_reap(
            start + std::time::Duration::from_secs(3),
            || now.get(),
            || GitReapPoll::Running,
            || now.set(now.get() + std::time::Duration::from_secs(1)),
        );
        assert_eq!(outcome, GitReapOutcome::TimedOut);
    }

    #[test]
    fn bounded_git_reap_observes_exit_during_the_last_sleep() {
        let start = std::time::Instant::now();
        let logical_ms = std::cell::Cell::new(15_u64);
        let outcome = bounded_reap(
            start + std::time::Duration::from_millis(20),
            || start + std::time::Duration::from_millis(logical_ms.get()),
            || {
                if logical_ms.get() >= 16 {
                    GitReapPoll::Exited
                } else {
                    GitReapPoll::Running
                }
            },
            || logical_ms.set(logical_ms.get() + 10),
        );
        assert_eq!(outcome, GitReapOutcome::Exited);
    }

    #[test]
    fn git_root_output_is_utf8_and_size_bounded() {
        assert_eq!(
            parse_git_root(b"C:/Users/alice/project\r\n"),
            Some(std::path::PathBuf::from("C:/Users/alice/project"))
        );
        assert!(parse_git_root(&vec![b'x'; GIT_STDOUT_MAX_BYTES + 1]).is_none());
    }

    #[test]
    fn git_stdout_accumulator_keeps_draining_after_the_output_cap() {
        let mut output = Vec::new();
        let chunks = (0..(GIT_STDOUT_MAX_BYTES / 4096 + 4)).map(|index| {
            if index == 0 {
                vec![b'a'; 4096]
            } else {
                vec![b'b'; 4096]
            }
        });

        for chunk in chunks {
            append_git_stdout(&mut output, &chunk);
        }

        assert_eq!(output.len(), GIT_STDOUT_MAX_BYTES + 1);
        assert_eq!(output[0], b'a');
        assert_eq!(output[GIT_STDOUT_MAX_BYTES], b'b');
    }

    #[test]
    fn detect_git_repository_classifies_a_real_root_and_nested_folder() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "devboule-git-detection-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root).expect("test directory");
        let init = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["init", "--quiet"])
            .output();
        let Ok(init) = init else {
            assert_eq!(detect_git_repository(&root), GitRepositoryStatus::Unknown);
            let _ = std::fs::remove_dir_all(&root);
            return;
        };
        assert!(init.status.success(), "git init failed");

        assert_eq!(
            detect_git_repository(&root),
            GitRepositoryStatus::RepositoryRoot
        );
        let nested = root.join("nested");
        std::fs::create_dir(&nested).expect("nested directory");
        assert_eq!(
            detect_git_repository(&nested),
            GitRepositoryStatus::InsideRepository
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "spawns PowerShell and cmd.exe; run by ignored-tests-informational on scheduled/dispatch Windows CI"]
    fn git_probe_drains_large_stdout_before_waiting_for_exit() {
        let root = unique_probe_directory("git-output");
        std::fs::create_dir(&root).expect("test directory");
        let script = root.join("emit.cmd");
        let powershell = root.join("emit.ps1");
        std::fs::write(
            &script,
            format!(
                "@echo off\r\npowershell.exe -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File \"{}\"\r\n",
                powershell.display()
            ),
        )
        .expect("batch probe");
        std::fs::write(&powershell, "[Console]::Out.Write(('x' * 20000))\r\n")
            .expect("powershell probe");

        let status = detect_git_repository_with_program(
            &root,
            Path::new(r"C:\Windows\System32\cmd.exe").as_os_str(),
            &[
                OsString::from("/d"),
                OsString::from("/c"),
                script.into_os_string(),
            ],
        );
        assert_eq!(status, GitRepositoryStatus::Unknown);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "spawns PowerShell, ping.exe, and a real process tree; run by ignored-tests-informational on scheduled/dispatch Windows CI"]
    fn timed_out_git_probe_leaves_no_descendant_process_alive() {
        let root = unique_probe_directory("git-tree");
        std::fs::create_dir(&root).expect("test directory");
        let script = root.join("spawn.cmd");
        let powershell = root.join("spawn.ps1");
        let marker = root.join("child.pid");
        std::fs::write(
            &script,
            format!(
                "@echo off\r\npowershell.exe -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File \"{}\"\r\n",
                powershell.display()
            ),
        )
        .expect("batch probe");
        std::fs::write(
            &powershell,
            format!(
                "$p = Start-Process -FilePath ping.exe -ArgumentList '-n','30','127.0.0.1' -PassThru -WindowStyle Hidden\r\nSet-Content -LiteralPath '{}' -Value @($PID, $p.Id)\r\n[Console]::Out.Write(('x' * 20000))\r\nWait-Process -Id $p.Id\r\n",
                marker.display()
            ),
        )
        .expect("powershell probe");

        let status = detect_git_repository_with_program(
            &root,
            Path::new(r"C:\Windows\System32\cmd.exe").as_os_str(),
            &[
                OsString::from("/d"),
                OsString::from("/c"),
                script.into_os_string(),
            ],
        );
        assert_eq!(status, GitRepositoryStatus::TimedOut);
        let child_pids: Vec<u32> = std::fs::read_to_string(&marker)
            .expect("descendant marker")
            .lines()
            .map(|line| line.trim().parse().expect("process pid"))
            .collect();
        assert_eq!(child_pids.len(), 2, "tracked process ids: {child_pids:?}");
        let alive_count = child_pids
            .iter()
            .filter(|child_pid| {
                let tasklist = std::process::Command::new("tasklist")
                    .args(["/FI", &format!("PID eq {child_pid}"), "/FO", "CSV", "/NH"])
                    .output()
                    .expect("tasklist");
                let listing = String::from_utf8_lossy(&tasklist.stdout);
                listing
                    .lines()
                    .any(|line| line.contains(&child_pid.to_string()))
            })
            .count();
        eprintln!(
            "git probe tracked process count={}, alive_after_timeout={alive_count}",
            child_pids.len()
        );
        assert_eq!(alive_count, 0, "Job Object left a tracked descendant alive");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    fn unique_probe_directory(label: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("devboule-{label}-{}-{stamp}", std::process::id()))
    }
}
