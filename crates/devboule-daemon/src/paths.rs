use std::path::{Path, PathBuf};

/// Runtime directory, lock file, and named-pipe name.
///
/// The pipe name is a deterministic FNV-1a of the runtime directory so two
/// processes of the same user agree without a side channel. `std` hashing is
/// not used: `DefaultHasher` is seeded per process and would split the
/// client and the daemon onto different pipes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimePaths {
    pub dir: PathBuf,
    pub lock_file: PathBuf,
    pub pipe_name: String,
}

impl RuntimePaths {
    pub fn from_env() -> std::io::Result<Self> {
        if let Some(dir) = std::env::var_os("DEVBOULE_RUNTIME_DIR") {
            return Ok(Self::from_dir(PathBuf::from(dir)));
        }
        let base = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "LOCALAPPDATA is not set; cannot place the daemon lock file",
            )
        })?;
        Ok(Self::from_dir(PathBuf::from(base).join("Devboule")))
    }

    pub fn from_dir(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let lock_file = dir.join("daemon.lock");
        let pipe_name = pipe_name_for(&dir);
        Self {
            dir,
            lock_file,
            pipe_name,
        }
    }

    pub fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)
    }

    /// SQLite WAL journal. Lives next to the lock file so a test runtime
    /// dir (including paths with spaces) owns its own database.
    pub fn journal_file(&self) -> PathBuf {
        self.dir.join("journal.db")
    }
}

fn pipe_name_for(dir: &Path) -> String {
    let mut normalized = dir.to_string_lossy().replace('/', "\\").to_lowercase();
    while normalized.ends_with('\\') {
        normalized.pop();
    }
    format!(
        "\\\\.\\pipe\\devboule-{:016x}",
        fnv1a64(normalized.as_bytes())
    )
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_name_is_stable_across_slash_and_case() {
        let a = RuntimePaths::from_dir(r"C:\Users\Gualt\AppData\Local\Devboule");
        let b = RuntimePaths::from_dir(r"c:/users/gualt/appdata/local/devboule/");
        assert_eq!(a.pipe_name, b.pipe_name);
        assert!(a.pipe_name.starts_with(r"\\.\pipe\devboule-"));
    }

    #[test]
    fn distinct_dirs_get_distinct_pipes() {
        let a = RuntimePaths::from_dir(r"C:\tmp\devboule one");
        let b = RuntimePaths::from_dir(r"C:\tmp\devboule two");
        assert_ne!(a.pipe_name, b.pipe_name);
    }

    #[test]
    fn lock_file_sits_inside_the_runtime_dir() {
        let paths = RuntimePaths::from_dir(r"C:\Users\Name With Spaces\AppData\Local\Devboule");
        assert_eq!(
            paths.lock_file,
            PathBuf::from(r"C:\Users\Name With Spaces\AppData\Local\Devboule\daemon.lock")
        );
        assert_eq!(
            paths.journal_file(),
            PathBuf::from(r"C:\Users\Name With Spaces\AppData\Local\Devboule\journal.db")
        );
    }
}
