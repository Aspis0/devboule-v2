//! Where the daemon finds its OpenCode Go key: the `OPENCODE_API_KEY`
//! environment variable first, then the `opencode` entry of Pi's own
//! `auth.json`. A key is read for one request at a time and leaves this module
//! only as the Authorization header of that request.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The variable Pi and OpenCode both name for the shared Zen/Go key.
const ENV_NAME: &str = "OPENCODE_API_KEY";
/// Pi's provider id for OpenCode inside `auth.json`.
const AUTH_PROVIDER: &str = "opencode";
/// Far larger than any credential file; a bigger file is not read.
const MAX_AUTH_BYTES: u64 = 1 << 20;

/// A key the daemon may put on a request. Its `Debug` says only that a key
/// exists, so a logged value never carries the text.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ApiKey(String);

impl ApiKey {
    /// A key from text, or none when the text is blank.
    pub(crate) fn from_text(text: &str) -> Option<Self> {
        let trimmed = text.trim();
        (!trimmed.is_empty()).then(|| Self(trimmed.to_string()))
    }

    /// The Authorization header value: the one place the key text leaves here.
    pub(crate) fn bearer(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(redacted)")
    }
}

/// Where a key can come from. Production hands in the process environment and
/// the person's home folder; tests hand in their own.
pub(crate) struct KeySources<'a> {
    pub(crate) env: &'a dyn Fn(&str) -> Option<String>,
    pub(crate) home: Option<PathBuf>,
}

/// Pi's `auth.json` under a home folder.
pub(crate) fn pi_auth_path(home: &Path) -> PathBuf {
    home.join(".pi").join("agent").join("auth.json")
}

/// Which source supplied the key. It names the source for the log and carries
/// no key text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeySource {
    Environment,
    PiAuthFile,
    Nothing,
}

impl KeySource {
    /// The INFO line written when the source changes. It names the source only,
    /// so no log line can carry the key.
    pub(crate) fn log_line(self) -> String {
        let source = match self {
            Self::Environment => "the OPENCODE_API_KEY environment variable",
            Self::PiAuthFile => "the opencode entry of Pi's auth.json",
            Self::Nothing => "no key",
        };
        format!("INFO opencode-go quota: key source is {source}")
    }
}

/// The environment variable first; a blank one falls through to the file.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn opencode_key(sources: &KeySources<'_>) -> Option<ApiKey> {
    opencode_key_and_source(sources).0
}

/// The key and the source that supplied it, or none and [`KeySource::Nothing`].
pub(crate) fn opencode_key_and_source(sources: &KeySources<'_>) -> (Option<ApiKey>, KeySource) {
    if let Some(key) = (sources.env)(ENV_NAME)
        .as_deref()
        .and_then(ApiKey::from_text)
    {
        return (Some(key), KeySource::Environment);
    }
    match sources
        .home
        .as_deref()
        .and_then(|home| key_from_auth_file(&pi_auth_path(home)))
    {
        Some(key) => (Some(key), KeySource::PiAuthFile),
        None => (None, KeySource::Nothing),
    }
}

/// The key and source the production sources name: this process's environment
/// and the person's home folder.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn opencode_key_from_process() -> (Option<ApiKey>, KeySource) {
    let env = |name: &str| std::env::var(name).ok();
    opencode_key_and_source(&KeySources {
        env: &env,
        home: home_dir(),
    })
}

/// The home folder: `USERPROFILE` on Windows, `HOME` elsewhere.
#[cfg_attr(test, allow(dead_code))]
fn home_dir() -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(name).map(PathBuf::from)
}

/// The key in Pi's `auth.json`, read only from a plain file that fits the cap.
///
/// `symlink_metadata` asks what the name is, not what it points at: a link, a
/// folder, or a dangling name is refused before a byte is read, so a
/// credential planted at the name never travels as this daemon's own. The
/// Windows attribute covers the reparse tags `FileType` does not call a
/// symlink, a junction among them.
///
/// The read stops one byte past the cap: an oversized file is refused whole,
/// never truncated into a shorter document that could still parse.
fn key_from_auth_file(path: &Path) -> Option<ApiKey> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT: the attribute every Windows link
        // carries, including the junction tag `FileType` has no name for.
        if meta.file_attributes() & 0x400 != 0 {
            return None;
        }
    }
    let file = std::fs::File::open(path).ok()?;
    let mut text = String::new();
    let mut capped = file.take(MAX_AUTH_BYTES + 1);
    let read = capped.read_to_string(&mut text).ok()?;
    if read as u64 > MAX_AUTH_BYTES {
        return None;
    }
    key_from_auth_text(&text)
}

/// The key in one `auth.json` text: the `opencode` entry, when it is an
/// `api_key` credential. A key written as `!command` needs a shell to resolve,
/// which this daemon does not run, so it is no key here.
fn key_from_auth_text(text: &str) -> Option<ApiKey> {
    let value: Value = serde_json::from_str(text).ok()?;
    let entry = value.get(AUTH_PROVIDER)?;
    if entry.get("type")?.as_str()? != "api_key" {
        return None;
    }
    let key = entry.get("key")?.as_str()?;
    if key.starts_with('!') {
        return None;
    }
    ApiKey::from_text(key)
}

#[cfg(test)]
#[path = "quota_key_tests.rs"]
mod tests;
