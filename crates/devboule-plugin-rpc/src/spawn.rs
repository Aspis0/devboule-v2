use std::path::Path;
#[cfg(not(windows))]
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

pub const PIPE_ENV: &str = "DEVBOULE_PLUGIN_PIPE";
pub const PLUGIN_ID_ENV: &str = "DEVBOULE_PLUGIN_ID";
pub const HOST_PID_ENV: &str = "DEVBOULE_PLUGIN_HOST_PID";

/// Store/path and embedder overrides are host-process configuration. A
/// plugin backend must derive all workspace data from its granted root.
pub const ORACLE_CHILD_OVERRIDE_ENV_VARS: &[&str] = &[
    "ORACLE_DIR",
    "ORACLE_QUERY_EMBEDDER",
    "ORACLE_REQUIRE_REAL_EMBEDDER",
    "ORACLE_EMBED_PROFILE",
    "LANCE_DB_PATH",
    "CHUNK_DB_PATH",
    "FILE_VECTORS_DB_PATH",
    "SQLITE_PATH",
    "CHUNK_MANIFEST_PATH",
    "CKG_DB_PATH",
];

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const CREATE_SUSPENDED: u32 = 0x0000_0004;
#[cfg(windows)]
const CREATE_UNICODE_ENVIRONMENT: u32 = 0x0000_0400;
static PIPE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// The plugin child plus the primary thread handle returned by
/// `CreateProcessW`. `std::process::Child` does not retain that thread
/// handle, so the Windows path owns both handles until the process exits.
#[derive(Debug)]
pub struct SpawnedBackend {
    #[cfg(windows)]
    process: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(windows)]
    primary_thread: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(windows)]
    pid: u32,
    #[cfg(not(windows))]
    child: Child,
}

// Windows kernel handles are process-wide capabilities and this owner never
// exposes them as references to caller memory. Moving the owner between the
// host's blocking worker threads is therefore equivalent to moving a Child.
#[cfg(windows)]
unsafe impl Send for SpawnedBackend {}
#[cfg(windows)]
unsafe impl Sync for SpawnedBackend {}

impl SpawnedBackend {
    pub fn id(&self) -> u32 {
        #[cfg(windows)]
        {
            self.pid
        }
        #[cfg(not(windows))]
        {
            self.child.id()
        }
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<u32>> {
        #[cfg(windows)]
        {
            const STILL_ACTIVE: u32 = 259;
            let mut exit_code = 0u32;
            let ok = unsafe {
                windows_sys::Win32::System::Threading::GetExitCodeProcess(
                    self.process,
                    &mut exit_code,
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok((exit_code != STILL_ACTIVE).then_some(exit_code))
        }
        #[cfg(not(windows))]
        {
            self.child
                .try_wait()
                .map(|status| status.map(|status| status.code().unwrap_or_default()))
        }
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            if self.try_wait()?.is_some() {
                return Ok(());
            }
            let terminated =
                unsafe { windows_sys::Win32::System::Threading::TerminateProcess(self.process, 1) };
            if terminated == 0 && self.try_wait()?.is_none() {
                return Err(std::io::Error::last_os_error());
            }
            self.wait()
        }
        #[cfg(not(windows))]
        {
            match self.child.kill() {
                Ok(()) => {
                    let _ = self.child.wait();
                    Ok(())
                }
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
                Err(error) => Err(error),
            }
        }
    }

    pub fn wait(&mut self) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            const WAIT_OBJECT_0: u32 = 0;
            const WAIT_FAILED: u32 = 0xFFFF_FFFF;
            let result = unsafe {
                windows_sys::Win32::System::Threading::WaitForSingleObject(
                    self.process,
                    0xFFFF_FFFF,
                )
            };
            if result == WAIT_FAILED || result != WAIT_OBJECT_0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            self.child.wait().map(|_| ())
        }
    }

    #[cfg(windows)]
    pub(crate) fn process_handle(&self) -> std::os::windows::io::RawHandle {
        self.process
    }

    #[cfg(windows)]
    pub(crate) fn primary_thread_handle(&self) -> std::os::windows::io::RawHandle {
        self.primary_thread
    }
}

#[cfg(windows)]
impl Drop for SpawnedBackend {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.primary_thread);
            windows_sys::Win32::Foundation::CloseHandle(self.process);
        }
    }
}

pub fn unique_pipe_name(plugin_id: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let n = PIPE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        r"\\.\pipe\devboule-plugin-{plugin_id}-{}-{nanos}-{n}",
        std::process::id()
    )
}

/// Spawn the plugin backend. No breakaway: the Job Object the caller
/// assigns this child to is what kills orphans when the host exits.
pub fn spawn_backend(
    binary: &Path,
    plugin_id: &str,
    pipe_name: &str,
    hang_ms: Option<u64>,
) -> std::io::Result<SpawnedBackend> {
    #[cfg(windows)]
    {
        return spawn_backend_windows(binary, plugin_id, pipe_name, hang_ms);
    }

    #[cfg(not(windows))]
    {
        let mut command = Command::new(binary);
        command
            .arg("--pipe")
            .arg(pipe_name)
            .env(PIPE_ENV, pipe_name)
            .env(PLUGIN_ID_ENV, plugin_id)
            .env(HOST_PID_ENV, std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(hang_ms) = hang_ms {
            command.env("DEVBOULE_PLUGIN_HANG_MS", hang_ms.to_string());
        }
        sanitize_backend_environment(&mut command);
        command.spawn().map(|child| SpawnedBackend { child })
    }
}

#[cfg(windows)]
fn spawn_backend_windows(
    binary: &Path,
    plugin_id: &str,
    pipe_name: &str,
    hang_ms: Option<u64>,
) -> std::io::Result<SpawnedBackend> {
    use std::collections::BTreeMap;
    use std::ffi::{OsStr, OsString};
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
    };

    let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    sanitize_backend_environment_map(&mut environment);
    environment.insert(OsString::from(PIPE_ENV), OsString::from(pipe_name));
    environment.insert(OsString::from(PLUGIN_ID_ENV), OsString::from(plugin_id));
    environment.insert(
        OsString::from(HOST_PID_ENV),
        std::process::id().to_string().into(),
    );
    if let Some(hang_ms) = hang_ms {
        environment.insert(
            OsString::from("DEVBOULE_PLUGIN_HANG_MS"),
            hang_ms.to_string().into(),
        );
    }

    let mut environment_block = Vec::new();
    for (key, value) in environment {
        environment_block.extend(key.encode_wide());
        environment_block.push('=' as u16);
        environment_block.extend(value.encode_wide());
        environment_block.push(0);
    }
    environment_block.push(0);

    let mut binary_wide: Vec<u16> = binary.as_os_str().encode_wide().chain(once(0)).collect();
    let mut command_line = Vec::new();
    append_quoted_argument(&mut command_line, binary.as_os_str());
    command_line.extend(" --pipe ".encode_utf16());
    append_quoted_argument(&mut command_line, OsStr::new(pipe_name));
    command_line.push(0);

    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut information: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let created = unsafe {
        CreateProcessW(
            binary_wide.as_mut_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            0,
            backend_creation_flags(),
            environment_block.as_ptr().cast(),
            null(),
            &startup,
            &mut information,
        )
    };
    if created == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(SpawnedBackend {
        process: information.hProcess,
        primary_thread: information.hThread,
        pid: information.dwProcessId,
    })
}

/// Remove inherited Oracle overrides from a child `Command` on the portable
/// process path. The Windows path sanitizes its explicit environment block.
#[cfg(not(windows))]
fn sanitize_backend_environment(command: &mut std::process::Command) {
    for key in ORACLE_CHILD_OVERRIDE_ENV_VARS {
        command.env_remove(key);
    }
}

fn sanitize_backend_environment_map(
    environment: &mut std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) {
    for key in ORACLE_CHILD_OVERRIDE_ENV_VARS {
        environment.remove(std::ffi::OsStr::new(key));
    }
}

#[cfg(windows)]
fn append_quoted_argument(output: &mut Vec<u16>, argument: &std::ffi::OsStr) {
    use std::os::windows::ffi::OsStrExt;

    output.push('"' as u16);
    let mut backslashes = 0usize;
    for unit in argument.encode_wide() {
        if unit == '\\' as u16 {
            backslashes += 1;
        } else if unit == '"' as u16 {
            output.extend(std::iter::repeat_n('\\' as u16, backslashes * 2 + 1));
            output.push('"' as u16);
            backslashes = 0;
        } else {
            output.extend(std::iter::repeat_n('\\' as u16, backslashes));
            output.push(unit);
            backslashes = 0;
        }
    }
    output.extend(std::iter::repeat_n('\\' as u16, backslashes * 2));
    output.push('"' as u16);
}

/// Hash a backend file using the same lowercase SHA-256 representation as a
/// verified plugin manifest.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub fn verify_file_digest(path: &Path, expected: &str) -> std::io::Result<()> {
    let actual = sha256_file(path)?;
    if actual != expected.trim().to_ascii_lowercase() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "backend digest mismatch: expected {}, got {}",
                expected, actual
            ),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn backend_creation_flags() -> u32 {
    CREATE_NO_WINDOW | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT
}

pub fn pipe_name_from_env_or_argv(args: &[String]) -> Option<String> {
    let mut index = 0usize;
    while index < args.len() {
        if args[index] == "--pipe" {
            return args.get(index + 1).cloned();
        }
        if let Some(value) = args[index].strip_prefix("--pipe=") {
            return Some(value.to_string());
        }
        index += 1;
    }
    std::env::var(PIPE_ENV)
        .ok()
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_pipe_names_are_windows_pipe_paths() {
        let a = unique_pipe_name("polis");
        let b = unique_pipe_name("polis");
        assert!(a.starts_with(r"\\.\pipe\devboule-plugin-polis-"));
        assert_ne!(a, b);
    }

    #[test]
    fn missing_backend_binary_fails_to_spawn() {
        let error = spawn_backend(
            std::path::Path::new("no-such-polis-backend.exe"),
            "polis",
            r"\\.\pipe\devboule-plugin-missing",
            None,
        )
        .expect_err("missing binary");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn tampered_backend_digest_is_refused() {
        let directory = tempfile::tempdir().expect("temp directory");
        let binary = directory.path().join("polis-backend.exe");
        std::fs::write(&binary, b"trusted backend").expect("trusted binary");
        let expected = sha256_file(&binary).expect("trusted digest");
        std::fs::write(&binary, b"tampered backend").expect("tampered binary");

        let error = verify_file_digest(&binary, &expected).expect_err("tampered binary");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("digest mismatch"));
    }

    #[test]
    fn backend_child_does_not_inherit_oracle_path_overrides() {
        #[cfg(not(windows))]
        {
            let mut command = std::process::Command::new("polis-backend.exe");
            for key in ORACLE_CHILD_OVERRIDE_ENV_VARS {
                command.env(key, "foreign-value");
            }
            sanitize_backend_environment(&mut command);
            for key in ORACLE_CHILD_OVERRIDE_ENV_VARS {
                assert!(
                    command.get_envs().all(|(name, value)| {
                        name != std::ffi::OsStr::new(key) || value.is_none()
                    }),
                    "{key} must be removed from the child environment"
                );
            }
        }

        let mut environment = std::collections::BTreeMap::new();
        for key in ORACLE_CHILD_OVERRIDE_ENV_VARS {
            environment.insert(std::ffi::OsString::from(key), std::ffi::OsString::from("foreign"));
        }
        sanitize_backend_environment_map(&mut environment);
        assert!(ORACLE_CHILD_OVERRIDE_ENV_VARS
            .iter()
            .all(|key| !environment.contains_key(std::ffi::OsStr::new(key))));
    }

    #[test]
    fn pipe_name_prefers_argv_then_env() {
        let args = vec![
            "polis-backend.exe".to_string(),
            "--pipe".to_string(),
            r"\\.\pipe\from-argv".to_string(),
        ];
        assert_eq!(
            pipe_name_from_env_or_argv(&args).as_deref(),
            Some(r"\\.\pipe\from-argv")
        );
    }

    #[cfg(windows)]
    #[test]
    fn backend_creation_is_suspended_before_job_assignment() {
        assert_ne!(backend_creation_flags() & CREATE_SUSPENDED, 0);
    }

    #[cfg(windows)]
    #[test]
    fn suspended_spawn_retains_create_process_primary_thread() {
        let mut child = spawn_backend(
            &std::env::current_exe().expect("test executable"),
            "thread-handle-test",
            r"\\.\pipe\devboule-plugin-thread-handle-test",
            None,
        )
        .expect("suspended child");
        assert!(!child.primary_thread_handle().is_null());
        child.kill().expect("kill suspended child");
    }
}
