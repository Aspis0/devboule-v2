//! ACP registry fetch, cache, and npx-only parse.
//!
//! Binary and uvx distributions are skipped: binary is third-party archive
//! download without a sandbox, uvx is out of scope for this slice.

use std::collections::{HashMap, HashSet};
use std::fs;
#[cfg(not(test))]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const REGISTRY_URL: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const NPM_REGISTRY_URL: &str = "https://registry.npmjs.org";
const CACHE_FILE: &str = "acp-registry-cache.json";
const MEMORY_TTL: Duration = Duration::from_secs(30 * 60);
const NPM_MEMORY_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_REGISTRY_BODY_BYTES: usize = 8 * 1024 * 1024;
#[cfg(not(test))]
const MAX_NPM_VERSION_BODY_BYTES: usize = 1024 * 1024;

type MemoryEntries = HashMap<PathBuf, (Instant, Vec<RegistryNpxEntry>)>;
type NpmVersionEntries = HashMap<String, (Instant, String)>;

/// In-process parsed entries keyed by cache dir. Within TTL, `load_npx_entries`
/// serves this map and does not fetch or read disk. Production uses one
/// runtime dir; the map is keyed so tests with unique temp dirs do not
/// clobber each other.
///
/// TODO: background refresh so the request path NEVER blocks on the CDN
/// fetch. This 30-minute TTL is the stopgap; a dedicated refresher should
/// populate the map off the `providers_list` / session-create path.
static MEMORY_CACHE: OnceLock<Mutex<MemoryEntries>> = OnceLock::new();
static NPM_VERSION_CACHE: OnceLock<Mutex<NpmVersionEntries>> = OnceLock::new();
static CACHE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn memory_cache() -> &'static Mutex<MemoryEntries> {
    MEMORY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Injectable registry body source so tests never touch the CDN.
pub trait RegistryFetch {
    fn fetch_body(&self) -> Result<String, String>;
}

/// Injectable npm metadata source so refresh tests never open a network socket.
pub trait NpmVersionFetch {
    fn latest(&self, package: &str) -> Result<String, String>;
}

/// Production CDN fetch. Under `cfg(test)` this fails immediately so the
/// suite never opens a network socket.
pub struct CdnRegistryFetch;

impl RegistryFetch for CdnRegistryFetch {
    fn fetch_body(&self) -> Result<String, String> {
        #[cfg(test)]
        {
            Err(format!(
                "registry fetch is disabled in tests ({REGISTRY_URL}, {}s)",
                FETCH_TIMEOUT.as_secs()
            ))
        }
        #[cfg(not(test))]
        {
            if std::env::var_os("DEVBOULE_TEST_NO_NETWORK").is_some() {
                return Err("network disabled by test harness".to_string());
            }
            fetch_registry_json()
        }
    }
}

/// Production npm registry fetch. Tests use a fake implementation instead.
pub struct CdnNpmVersionFetch;

impl NpmVersionFetch for CdnNpmVersionFetch {
    fn latest(&self, package: &str) -> Result<String, String> {
        #[cfg(test)]
        {
            let _ = package;
            Err(format!(
                "npm version fetch is disabled in tests ({NPM_REGISTRY_URL})"
            ))
        }
        #[cfg(not(test))]
        {
            if std::env::var_os("DEVBOULE_TEST_NO_NETWORK").is_some() {
                return Err("network disabled by test harness".to_string());
            }
            fetch_npm_latest(package)
        }
    }
}

#[cfg(not(test))]
fn fetch_registry_json() -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(REGISTRY_URL)
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("registry HTTP {}", response.status()));
    }
    let mut body = Vec::new();
    response
        .take((MAX_REGISTRY_BODY_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|error| error.to_string())?;
    let body = String::from_utf8(body).map_err(|error| error.to_string())?;
    bounded_registry_body(body)
}

fn bounded_registry_body(body: String) -> Result<String, String> {
    if body.len() > MAX_REGISTRY_BODY_BYTES {
        return Err(format!(
            "registry response exceeds {MAX_REGISTRY_BODY_BYTES} bytes"
        ));
    }
    Ok(body)
}

#[cfg(not(test))]
fn fetch_npm_latest(package: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let url = format!(
        "{NPM_REGISTRY_URL}/{}/latest",
        encode_url_component(package)
    );
    let mut response = client.get(url).send().map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("npm registry HTTP {}", response.status()));
    }
    let mut body = Vec::new();
    response
        .by_ref()
        .take((MAX_NPM_VERSION_BODY_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|error| error.to_string())?;
    if body.len() > MAX_NPM_VERSION_BODY_BYTES {
        return Err(format!(
            "npm registry response exceeds {MAX_NPM_VERSION_BODY_BYTES} bytes"
        ));
    }
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|error| error.to_string())?;
    value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .and_then(crate::provider_catalog::cap_external_version)
        .ok_or_else(|| "npm registry response has no version".to_string())
}

#[cfg(not(test))]
fn encode_url_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

/// One npx-capable registry agent. `package` is the registry's package
/// field as written (`name@version`); `args` are the registry's args array.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryNpxEntry {
    pub id: String,
    pub package: String,
    pub args: Vec<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct CacheFile {
    #[serde(rename = "fetchedAtMs")]
    fetched_at_ms: u64,
    registry: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
struct RegistryDocument {
    #[serde(default)]
    agents: Vec<RegistryAgent>,
}

#[derive(Debug, serde::Deserialize)]
struct RegistryAgent {
    id: String,
    #[serde(default)]
    distribution: RegistryDistribution,
}

#[derive(Debug, Default, serde::Deserialize)]
struct RegistryDistribution {
    npx: Option<RegistryNpx>,
}

#[derive(Debug, serde::Deserialize)]
struct RegistryNpx {
    package: String,
    #[serde(default)]
    args: Vec<String>,
}

/// `name@version` or `@scope/name@version`. Rejects leading `-` and
/// anything with whitespace, so a registry `package` cannot become extra
/// npx flags.
fn is_safe_npx_package(package: &str) -> bool {
    if package.starts_with('-') || package.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((name, version)) = split_npx_name_version(package) else {
        return false;
    };
    is_npx_name(name) && is_npx_version(version)
}

fn split_npx_name_version(package: &str) -> Option<(&str, &str)> {
    if let Some(rest) = package.strip_prefix('@') {
        let slash = rest.find('/')?;
        let after_scope = &rest[slash + 1..];
        let at = after_scope.find('@')?;
        if at == 0 {
            return None;
        }
        Some((&package[..1 + slash + 1 + at], &after_scope[at + 1..]))
    } else {
        let at = package.find('@')?;
        if at == 0 {
            return None;
        }
        Some((&package[..at], &package[at + 1..]))
    }
}

/// Return the version suffix from `name@version`, including scoped names.
pub(crate) fn split_package_version(package: &str) -> Option<&str> {
    let (name, version) = package.rsplit_once('@')?;
    (!name.is_empty() && !version.is_empty()).then_some(version)
}

fn is_npx_name_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

fn is_npx_name(name: &str) -> bool {
    if let Some(rest) = name.strip_prefix('@') {
        let Some((scope, package)) = rest.split_once('/') else {
            return false;
        };
        is_npx_name_segment(scope) && is_npx_name_segment(package)
    } else {
        is_npx_name_segment(name)
    }
}

fn is_npx_version(version: &str) -> bool {
    let mut chars = version.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

fn args_are_safe(args: &[String]) -> bool {
    args.iter().all(|arg| !arg.chars().any(char::is_whitespace))
}

/// Parse npx rows from a registry JSON body. Binary and uvx agents are
/// omitted. Unknown extra fields are ignored. Hostile `package`/`args`
/// rows and duplicate ids (after ascii-lowercase) are skipped.
pub fn parse_npx_entries(json: &str) -> Vec<RegistryNpxEntry> {
    let Ok(document) = serde_json::from_str::<RegistryDocument>(json) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    document
        .agents
        .into_iter()
        .filter_map(|agent| {
            let npx = agent.distribution.npx?;
            if agent.id.is_empty() || npx.package.is_empty() {
                return None;
            }
            if !is_safe_npx_package(&npx.package) || !args_are_safe(&npx.args) {
                return None;
            }
            let id = agent.id.to_ascii_lowercase();
            if !seen.insert(id.clone()) {
                return None;
            }
            Some(RegistryNpxEntry {
                id,
                package: npx.package,
                args: npx.args,
            })
        })
        .collect()
}

fn memory_get(cache_dir: &Path) -> Option<Vec<RegistryNpxEntry>> {
    let guard = memory_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (fetched_at, entries) = guard.get(cache_dir)?;
    if fetched_at.elapsed() < MEMORY_TTL {
        Some(entries.clone())
    } else {
        None
    }
}

fn memory_put(cache_dir: &Path, entries: Vec<RegistryNpxEntry>) {
    let mut guard = memory_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.insert(cache_dir.to_path_buf(), (Instant::now(), entries));
}

// Per-key on purpose: tests run in parallel and the caches are process-global,
// so a whole-map clear() from one test races the entry another test just
// seeded (measured on CI: failed_forced_refresh_keeps_the_good_in_process_cache
// saw its memory entry vanish and fell back to disk).
#[cfg(test)]
pub(crate) fn reset_memory_cache(cache_dir: &Path) {
    memory_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(cache_dir);
}

/// Fetch (or fall back to cache) and return npx entries. Fetch failure with
/// no cache is an empty contribution, not an error.
///
/// TODO: background refresh so the request path NEVER blocks on the CDN
/// fetch. Within `MEMORY_TTL` this serves the in-process map only.
pub fn load_npx_entries(fetch: &dyn RegistryFetch, cache_dir: &Path) -> Vec<RegistryNpxEntry> {
    load_npx_entries_with_force(fetch, cache_dir, false)
}

/// Fetch registry data regardless of the in-process TTL, retaining disk-cache
/// fallback if the explicit refresh cannot reach the registry.
pub fn refresh_npx_entries(fetch: &dyn RegistryFetch, cache_dir: &Path) -> Vec<RegistryNpxEntry> {
    load_npx_entries_with_force(fetch, cache_dir, true)
}

fn load_npx_entries_with_force(
    fetch: &dyn RegistryFetch,
    cache_dir: &Path,
    force: bool,
) -> Vec<RegistryNpxEntry> {
    if !force {
        if let Some(entries) = memory_get(cache_dir) {
            return entries;
        }
    }
    let (entries, fetched) = match fetch.fetch_body().and_then(bounded_registry_body) {
        Ok(body) => {
            write_cache(cache_dir, &body);
            (parse_npx_entries(&body), true)
        }
        Err(_) => match memory_get(cache_dir) {
            Some(entries) => (entries, false),
            None => (
                read_cache(cache_dir)
                    .map(|body| parse_npx_entries(&body))
                    .unwrap_or_default(),
                true,
            ),
        },
    };
    if fetched {
        memory_put(cache_dir, entries.clone());
    }
    entries
}

fn npm_version_cache() -> &'static Mutex<NpmVersionEntries> {
    NPM_VERSION_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Return cached npm metadata without contacting the registry.
pub(crate) fn cached_latest_npm_version(package: &str) -> Option<String> {
    let guard = npm_version_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (fetched_at, version) = guard.get(package)?;
    (fetched_at.elapsed() < NPM_MEMORY_TTL).then(|| version.clone())
}

/// Load npm metadata, optionally bypassing the six-hour in-process cache.
pub(crate) fn load_latest_npm_version(
    fetch: &dyn NpmVersionFetch,
    package: &str,
    force: bool,
) -> Option<String> {
    if !force {
        if let Some(version) = cached_latest_npm_version(package) {
            return Some(version);
        }
    }
    let version = match fetch
        .latest(package)
        .ok()
        .and_then(|version| crate::provider_catalog::cap_external_version(&version))
    {
        Some(version) => version,
        None => return cached_latest_npm_version(package),
    };
    npm_version_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(package.to_string(), (Instant::now(), version.clone()));
    Some(version)
}

// Per-key for the same reason as reset_memory_cache: clear() would race
// parallel tests sharing this process-global map.
#[cfg(test)]
pub(crate) fn reset_npm_version_cache(package: &str) {
    npm_version_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(package);
}

pub(crate) fn write_cache(cache_dir: &Path, body: &str) {
    let Ok(registry) = serde_json::from_str::<serde_json::Value>(body) else {
        return;
    };
    let _ = fs::create_dir_all(cache_dir);
    let fetched_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    let encoded = serde_json::to_vec(&CacheFile {
        fetched_at_ms,
        registry,
    });
    if let Ok(bytes) = encoded {
        let target = cache_dir.join(CACHE_FILE);
        let temp = cache_dir.join(format!(
            ".{CACHE_FILE}.tmp-{}-{}",
            std::process::id(),
            CACHE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        if fs::write(&temp, bytes).is_ok() {
            if fs::rename(&temp, &target).is_err() {
                let _ = fs::remove_file(temp);
            }
        } else {
            let _ = fs::remove_file(temp);
        }
    }
}

fn read_cache(cache_dir: &Path) -> Option<String> {
    let bytes = fs::read(cache_dir.join(CACHE_FILE)).ok()?;
    let cache: CacheFile = serde_json::from_slice(&bytes).ok()?;
    serde_json::to_string(&cache.registry).ok()
}

#[cfg(test)]
pub(crate) const TEST_REGISTRY_FIXTURE: &str = r#"{
  "version": "1.0.0",
  "agents": [
    {
      "id": "codex-acp",
      "name": "Codex",
      "license": "Apache-2.0",
      "distribution": {
        "npx": {
          "package": "@agentclientprotocol/codex-acp@1.10.0"
        }
      }
    },
    {
      "id": "grok-build",
      "distribution": {
        "npx": {
          "package": "@xai-official/grok@1.0.21",
          "args": ["agent", "stdio"]
        }
      }
    },
    {
      "id": "qwen-code",
      "distribution": {
        "npx": {
          "package": "@qwen-code/qwen-code@0.23.0",
          "args": ["--acp", "--experimental-skills"]
        }
      }
    },
    {
      "id": "amp-acp",
      "distribution": {
        "binary": {
          "windows-x86_64": {
            "archive": "https://example.invalid/amp.zip",
            "cmd": "amp-acp.exe"
          }
        }
      }
    },
    {
      "id": "fast-agent",
      "distribution": {
        "uvx": {
          "package": "fast-agent-acp==0.10.1",
          "args": ["-x"]
        }
      }
    }
  ]
}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct FakeFetch {
        result: Result<String, String>,
    }

    impl RegistryFetch for FakeFetch {
        fn fetch_body(&self) -> Result<String, String> {
            self.result.clone()
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "devboule-registry-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn parse_keeps_npx_rows_and_skips_binary_and_uvx() {
        let entries = parse_npx_entries(TEST_REGISTRY_FIXTURE);
        let ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, ["codex-acp", "grok-build", "qwen-code"]);
        assert_eq!(entries[0].package, "@agentclientprotocol/codex-acp@1.10.0");
        assert!(entries[0].args.is_empty());
        assert_eq!(entries[1].args, ["agent", "stdio"]);
        assert_eq!(entries[2].args, ["--acp", "--experimental-skills"]);
    }

    #[test]
    fn parse_skips_hostile_package_and_whitespace_args() {
        let entries = parse_npx_entries(
            r#"{
  "agents": [
    {
      "id": "evil-npx",
      "distribution": {
        "npx": {
          "package": "--registry=https://evil.invalid pkg@1.0.0"
        }
      }
    },
    {
      "id": "evil-args",
      "distribution": {
        "npx": {
          "package": "legit@1.0.0",
          "args": ["--acp", "foo bar"]
        }
      }
    },
    {
      "id": "evil-newline",
      "distribution": {
        "npx": {
          "package": "legit@1.0.0",
          "args": ["--acp\n--inject"]
        }
      }
    },
    {
      "id": "good-npx",
      "distribution": {
        "npx": {
          "package": "good-pkg@1.0.0",
          "args": ["--acp"]
        }
      }
    },
    {
      "id": "scoped-ok",
      "distribution": {
        "npx": {
          "package": "@agentclientprotocol/codex-acp@1.10.0"
        }
      }
    }
  ]
}"#,
        );
        let ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, ["good-npx", "scoped-ok"]);
        assert_eq!(entries[0].args, ["--acp"]);
    }

    #[test]
    fn parse_normalizes_registry_ids_to_ascii_lowercase() {
        let entries = parse_npx_entries(
            r#"{
  "agents": [
    {
      "id": "Codex-ACP",
      "distribution": {
        "npx": { "package": "codex-acp@1.0.0" }
      }
    }
  ]
}"#,
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "codex-acp");
    }

    #[test]
    fn parse_invalid_json_yields_empty() {
        assert!(parse_npx_entries("not-json").is_empty());
        assert!(parse_npx_entries("{").is_empty());
        assert!(parse_npx_entries("[]").is_empty());
    }

    #[test]
    fn parse_skips_empty_id_and_empty_package() {
        let entries = parse_npx_entries(
            r#"{
  "agents": [
    {
      "id": "",
      "distribution": {
        "npx": { "package": "empty-id@1.0.0" }
      }
    },
    {
      "id": "empty-package",
      "distribution": {
        "npx": { "package": "" }
      }
    },
    {
      "id": "kept",
      "distribution": {
        "npx": { "package": "kept@1.0.0" }
      }
    }
  ]
}"#,
        );
        let ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, ["kept"]);
    }

    #[test]
    fn parse_dedups_registry_ids_keeping_the_first() {
        let entries = parse_npx_entries(
            r#"{
  "agents": [
    {
      "id": "dup-agent",
      "distribution": {
        "npx": { "package": "first-pkg@1.0.0" }
      }
    },
    {
      "id": "dup-agent",
      "distribution": {
        "npx": { "package": "second-pkg@2.0.0" }
      }
    }
  ]
}"#,
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "dup-agent");
        assert_eq!(entries[0].package, "first-pkg@1.0.0");
    }

    #[test]
    fn load_uses_cache_when_fetch_fails() {
        let dir = temp_dir("cache-hit");
        fs::create_dir_all(&dir).expect("cache dir");
        write_cache(&dir, TEST_REGISTRY_FIXTURE);
        let entries = load_npx_entries(
            &FakeFetch {
                result: Err("cdn down".to_string()),
            },
            &dir,
        );
        let ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, ["codex-acp", "grok-build", "qwen-code"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_is_empty_when_fetch_fails_and_nothing_is_cached() {
        let dir = temp_dir("cache-miss");
        let entries = load_npx_entries(
            &FakeFetch {
                result: Err("cdn down".to_string()),
            },
            &dir,
        );
        assert!(entries.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    struct CountingFetch {
        body: String,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl RegistryFetch for CountingFetch {
        fn fetch_body(&self) -> Result<String, String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.body.clone())
        }
    }

    #[test]
    fn load_serves_in_process_memory_within_ttl_without_a_second_fetch() {
        let dir = temp_dir("ttl-memory");
        reset_memory_cache(&dir);
        let fetch = CountingFetch {
            body: TEST_REGISTRY_FIXTURE.to_string(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let first = load_npx_entries(&fetch, &dir);
        let second = load_npx_entries(&fetch, &dir);
        assert_eq!(
            fetch.calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "two loads within TTL must not fetch twice"
        );
        assert_eq!(first, second);
        assert!(!first.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_rejects_an_oversized_fetch_before_parsing_or_caching() {
        let dir = temp_dir("oversized-fetch");
        reset_memory_cache(&dir);
        let mut body = r#"{
  "agents": [
    {
      "id": "oversized",
      "distribution": { "npx": { "package": "oversized@1.0.0" } }
    }
  ]
}"#
        .to_string();
        body.push_str(&" ".repeat(MAX_REGISTRY_BODY_BYTES + 1));

        let entries = load_npx_entries(&FakeFetch { result: Ok(body) }, &dir);

        assert!(entries.is_empty());
        assert!(!dir.join(CACHE_FILE).exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn write_cache_round_trips_after_replace() {
        let dir = temp_dir("atomic-write");
        write_cache(&dir, TEST_REGISTRY_FIXTURE);
        write_cache(
            &dir,
            r#"{
  "agents": [
    {
      "id": "replacement",
      "distribution": {
        "npx": { "package": "replacement@1.0.0" }
      }
    }
  ]
}"#,
        );
        let entries = load_npx_entries(
            &FakeFetch {
                result: Err("cdn down".to_string()),
            },
            &dir,
        );
        let ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, ["replacement"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn split_package_version_uses_the_last_at_for_scoped_packages() {
        assert_eq!(
            split_package_version("@xai-official/grok@1.0.21"),
            Some("1.0.21")
        );
        assert_eq!(split_package_version("plain-package@2.3.4"), Some("2.3.4"));
        assert_eq!(split_package_version("no-version"), None);
    }

    #[test]
    fn forced_refresh_refetches_while_plain_load_uses_memory_ttl() {
        let dir = temp_dir("force-refresh");
        reset_memory_cache(&dir);
        let fetch = CountingFetch {
            body: TEST_REGISTRY_FIXTURE.to_string(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let _ = load_npx_entries(&fetch, &dir);
        let _ = load_npx_entries(&fetch, &dir);
        let _ = refresh_npx_entries(&fetch, &dir);
        assert_eq!(
            fetch.calls.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "forced refresh must bypass the in-process TTL"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_forced_refresh_keeps_the_good_in_process_cache() {
        let dir = temp_dir("force-refresh-failure");
        reset_memory_cache(&dir);
        let initial = r#"{
  "agents": [{
    "id": "cached-agent",
    "distribution": { "npx": { "package": "cached-agent@1.0.0" } }
  }]
}"#;
        let replacement = r#"{
  "agents": [{
    "id": "disk-agent",
    "distribution": { "npx": { "package": "disk-agent@2.0.0" } }
  }]
}"#;
        let _ = load_npx_entries(
            &FakeFetch {
                result: Ok(initial.to_string()),
            },
            &dir,
        );
        write_cache(&dir, replacement);

        let entries = refresh_npx_entries(
            &FakeFetch {
                result: Err("cdn down".to_string()),
            },
            &dir,
        );

        assert_eq!(entries[0].id, "cached-agent");
        assert_eq!(entries[0].package, "cached-agent@1.0.0");
        let _ = fs::remove_dir_all(dir);
    }

    struct FakeNpmFetch {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl NpmVersionFetch for FakeNpmFetch {
        fn latest(&self, _package: &str) -> Result<String, String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("9.9.9".to_string())
        }
    }

    #[test]
    fn npm_latest_cache_hits_within_ttl_and_force_bypasses_it() {
        reset_npm_version_cache("@scope/pkg");
        let fetch = FakeNpmFetch {
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        assert_eq!(
            load_latest_npm_version(&fetch, "@scope/pkg", false),
            Some("9.9.9".to_string())
        );
        assert_eq!(
            load_latest_npm_version(&fetch, "@scope/pkg", false),
            Some("9.9.9".to_string())
        );
        assert_eq!(
            load_latest_npm_version(&fetch, "@scope/pkg", true),
            Some("9.9.9".to_string())
        );
        assert_eq!(fetch.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn external_versions_are_capped_in_unicode_characters() {
        let version = format!("{}終", "x".repeat(64));
        assert_eq!(
            crate::provider_catalog::cap_external_version(&version)
                .unwrap()
                .chars()
                .count(),
            64
        );
        assert_eq!(
            crate::provider_catalog::cap_external_version(&version)
                .unwrap()
                .chars()
                .last(),
            Some('x')
        );
        assert_eq!(
            crate::provider_catalog::cap_external_version("1.2.3"),
            Some("1.2.3".to_string())
        );
    }
}
