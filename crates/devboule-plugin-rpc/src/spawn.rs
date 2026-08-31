use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const PIPE_ENV: &str = "DEVBOULE_PLUGIN_PIPE";
pub const PLUGIN_ID_ENV: &str = "DEVBOULE_PLUGIN_ID";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
static PIPE_COUNTER: AtomicU64 = AtomicU64::new(1);

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
) -> std::io::Result<Child> {
    let mut command = Command::new(binary);
    command
        .arg("--pipe")
        .arg(pipe_name)
        .env(PIPE_ENV, pipe_name)
        .env(PLUGIN_ID_ENV, plugin_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(hang_ms) = hang_ms {
        command.env("DEVBOULE_PLUGIN_HANG_MS", hang_ms.to_string());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn()
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
    std::env::var(PIPE_ENV).ok().filter(|value| !value.is_empty())
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
}
