//! Claude model catalog derived from the local CLI bundle.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use devboule_protocol::{SessionEvent, SessionModel, SessionModelEffort};

const CACHE_FILE: &str = "claude-model-catalog-cache.json";
const WINDOW_BYTES: usize = 4 * 1024 * 1024;
const OVERLAP_BYTES: usize = 1024 * 1024;
const RECORD_PREFIX: &[u8] = b"{id:\"claude-";
const FALLBACK_MODEL_IDS: &[(&str, &str)] = &[
    ("opus", "Claude Opus"),
    ("sonnet", "Claude Sonnet"),
    ("haiku", "Claude Haiku"),
];
const PERSISTED_EFFORTS: &[(&str, &str)] = &[
    // `max` is a registry/UI capability, not part of Claude's persisted
    // effortLevel enum, so it must never be offered to apply_flag_settings.
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("xhigh", "Extra High"),
];

static CACHE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
type MemoryKey = (PathBuf, String);
type ModelMemory = HashMap<MemoryKey, Vec<SessionModel>>;

static MEMORY_CACHE: OnceLock<Mutex<ModelMemory>> = OnceLock::new();
static IN_FLIGHT: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct CacheFile {
    #[serde(rename = "fetchedAtMs")]
    fetched_at_ms: u64,
    #[serde(rename = "cliVersion")]
    cli_version: String,
    #[serde(default)]
    derived: bool,
    models: Vec<SessionModel>,
}

pub(crate) trait CatalogSource: Send + Sync {
    fn derive(&self) -> Result<Vec<SessionModel>, String>;
}

struct ExecutableCatalogSource {
    path: PathBuf,
    portable: bool,
}

struct TestCatalogSource {
    models: Vec<SessionModel>,
}

impl CatalogSource for ExecutableCatalogSource {
    fn derive(&self) -> Result<Vec<SessionModel>, String> {
        if self.portable {
            scrape_file(&self.path)
        } else {
            derive_from_executable(&self.path)
        }
    }
}

impl CatalogSource for TestCatalogSource {
    fn derive(&self) -> Result<Vec<SessionModel>, String> {
        Ok(self.models.clone())
    }
}

pub(crate) fn source_for(executable: &Path) -> Arc<dyn CatalogSource> {
    // The integration harness already sets this marker to disable external
    // sources; make the local executable source fake as well.
    if std::env::var_os("DEVBOULE_TEST_NO_NETWORK").is_some() {
        return Arc::new(TestCatalogSource {
            models: fallback_models(),
        });
    }
    Arc::new(ExecutableCatalogSource {
        path: executable.to_path_buf(),
        portable: false,
    })
}

pub(crate) fn source_for_script(script: &Path) -> Arc<dyn CatalogSource> {
    if std::env::var_os("DEVBOULE_TEST_NO_NETWORK").is_some() {
        return Arc::new(TestCatalogSource {
            models: fallback_models(),
        });
    }
    Arc::new(ExecutableCatalogSource {
        path: script.to_path_buf(),
        portable: true,
    })
}

pub(crate) fn cached(runtime_dir: &Path, cli_version: &str) -> Option<Vec<SessionModel>> {
    let key = (runtime_dir.to_path_buf(), cli_version.to_string());
    if let Some(models) = memory_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .cloned()
    {
        return Some(models);
    }

    let models = read_cache(runtime_dir, cli_version)?;
    memory_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, models.clone());
    Some(models)
}

pub(crate) fn start_derivation(
    source: Arc<dyn CatalogSource>,
    runtime_dir: PathBuf,
    cli_version: String,
    on_complete: impl FnOnce(Vec<SessionModel>) + Send + 'static,
) -> bool {
    if cached(&runtime_dir, &cli_version).is_some() {
        return false;
    }
    let key = (runtime_dir.clone(), cli_version.clone());
    let job_key = runtime_dir.clone();
    let mut in_flight = IN_FLIGHT
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !in_flight.insert(job_key.clone()) {
        return false;
    }
    drop(in_flight);

    let worker_key = key.clone();
    let cleanup_key = job_key.clone();
    let spawn = std::thread::Builder::new()
        .name("claude-model-catalog".to_string())
        .spawn(move || {
            let (models, should_cache) = match cached(&runtime_dir, &cli_version) {
                Some(models) => (Some(models), false),
                None => (
                    source.derive().ok().filter(|models| !models.is_empty()),
                    true,
                ),
            };
            let Some(models) = models else {
                IN_FLIGHT
                    .get_or_init(|| Mutex::new(HashSet::new()))
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&cleanup_key);
                eprintln!(
                    "Claude model catalog derivation failed for CLI version {cli_version}; retrying later"
                );
                return;
            };
            if should_cache {
                let _ = write_cache(&runtime_dir, &cli_version, &models);
            }
            memory_cache()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(worker_key, models.clone());
            IN_FLIGHT
                .get_or_init(|| Mutex::new(HashSet::new()))
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&cleanup_key);
            on_complete(models);
        });
    if spawn.is_err() {
        IN_FLIGHT
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&job_key);
        return false;
    }
    true
}

pub(crate) fn manifest_with_current(
    models: Vec<SessionModel>,
    current_model_id: Option<String>,
) -> SessionEvent {
    let current_model_id = current_model_id.or_else(|| {
        models
            .iter()
            .find(|model| model.model_id == "claude-sonnet-5")
            .or_else(|| models.first())
            .map(|model| model.model_id.clone())
    });
    SessionEvent::SessionManifest {
        provider_id: Some("claude".to_string()),
        current_model_id,
        models,
        modes: None,
    }
}

pub(crate) fn initial_manifest(models: Vec<SessionModel>) -> SessionEvent {
    manifest_with_current(models, None)
}

pub(crate) fn fallback_models() -> Vec<SessionModel> {
    FALLBACK_MODEL_IDS
        .iter()
        .map(|(model_id, name)| SessionModel {
            model_id: (*model_id).to_string(),
            name: (*name).to_string(),
            description: Some("Claude model alias; the full catalog was unavailable.".to_string()),
            context_tokens: None,
            current_effort: Some("high".to_string()),
            efforts: Some(efforts(false, "high")),
        })
        .collect()
}

fn memory_cache() -> &'static Mutex<ModelMemory> {
    MEMORY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn derive_from_executable(path: &Path) -> Result<Vec<SessionModel>, String> {
    #[cfg(windows)]
    {
        scrape_file(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("Claude bundle scraping is unavailable on this platform".to_string())
    }
}

fn scrape_file(path: &Path) -> Result<Vec<SessionModel>, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    let started = Instant::now();
    let mut offset = 0u64;
    let mut ids = std::collections::HashSet::new();
    let mut models = Vec::new();
    let mut windows = 0u64;

    while offset < length {
        let start = offset.saturating_sub(OVERLAP_BYTES as u64);
        let bytes_to_read = (length - start).min((WINDOW_BYTES + OVERLAP_BYTES) as u64) as usize;
        file.seek(SeekFrom::Start(start))
            .map_err(|error| error.to_string())?;
        let mut buffer = vec![0u8; bytes_to_read];
        file.read_exact(&mut buffer)
            .map_err(|error| error.to_string())?;
        windows += 1;
        for record in records_in_window(&buffer) {
            let Some(model) = parse_model_record(record) else {
                continue;
            };
            if ids.insert(model.model_id.clone()) {
                models.push(model);
            }
        }
        offset = offset.saturating_add(WINDOW_BYTES as u64);
    }

    eprintln!(
        "derived Claude model catalog from {} in {} ms ({} windows, max {} bytes)",
        path.display(),
        started.elapsed().as_millis(),
        windows,
        WINDOW_BYTES + OVERLAP_BYTES
    );
    Ok(models)
}

fn records_in_window(buffer: &[u8]) -> Vec<&[u8]> {
    let mut records = Vec::new();
    let mut cursor = 0;
    while let Some(start) = next_record_start(buffer, cursor) {
        let Some(end) = balanced_object_end(&buffer[start..]) else {
            cursor = start + RECORD_PREFIX.len();
            continue;
        };
        records.push(&buffer[start..start + end]);
        cursor = start + end;
    }
    records
}

fn next_record_start(buffer: &[u8], mut cursor: usize) -> Option<usize> {
    while let Some(relative) = buffer[cursor..].iter().position(|byte| *byte == b'{') {
        let start = cursor + relative;
        if buffer[start..].starts_with(RECORD_PREFIX) {
            return Some(start);
        }
        cursor = start + 1;
    }
    None
}

fn balanced_object_end(record: &[u8]) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in record.iter().copied().enumerate() {
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == delimiter {
                quote = None;
            }
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'{' => depth += 1,
            b'}' if depth == 1 => return Some(index + 1),
            b'}' if depth > 1 => depth -= 1,
            _ => {}
        }
    }
    None
}

fn parse_model_record(record: &[u8]) -> Option<SessionModel> {
    let record = std::str::from_utf8(record).ok()?;
    let model_id = js_field(record, "id")?;
    if !model_id.starts_with("claude-") {
        return None;
    }
    let family = js_field(record, "family")?;
    let display_name = js_field(record, "display_name")?;
    let capabilities = js_array(record, "capabilities");
    let supports_effort = capabilities.iter().any(|capability| capability == "effort");
    let supports_xhigh = capabilities
        .iter()
        .any(|capability| capability == "xhigh_effort");
    let default_effort = js_field(record, "default_effort").filter(|effort| {
        supports_effort
            && PERSISTED_EFFORTS
                .iter()
                .any(|(persisted, _)| persisted == effort)
            && (effort != "xhigh" || supports_xhigh)
    });
    let efforts =
        supports_effort.then(|| efforts(supports_xhigh, default_effort.as_deref().unwrap_or("")));
    let knowledge_cutoff = js_field(record, "knowledge_cutoff");
    let description = knowledge_cutoff
        .map(|cutoff| format!("{family} family; knowledge cutoff {cutoff}"))
        .or(Some(family));

    Some(SessionModel {
        model_id,
        name: display_name,
        description,
        context_tokens: js_number(record, "window"),
        current_effort: default_effort,
        efforts,
    })
}

fn efforts(include_xhigh: bool, default_effort: &str) -> Vec<SessionModelEffort> {
    PERSISTED_EFFORTS
        .iter()
        .filter(|(id, _)| *id != "xhigh" || include_xhigh)
        .map(|(id, label)| SessionModelEffort {
            id: (*id).to_string(),
            label: (*label).to_string(),
            description: None,
            default: Some(*id == default_effort),
        })
        .collect()
}

fn js_field(record: &str, field: &str) -> Option<String> {
    let start = field_start(record, field)?;
    let quote = record.as_bytes().get(start).copied()?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let mut escaped = false;
    for (offset, byte) in record.as_bytes()[start + 1..].iter().copied().enumerate() {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == quote {
            let raw = &record[start..start + offset + 2];
            return if quote == b'"' {
                serde_json::from_str(raw).ok()
            } else {
                Some(raw[1..raw.len() - 1].replace("\\'", "'"))
            };
        }
    }
    None
}

fn js_array(record: &str, field: &str) -> Vec<String> {
    let Some(start) = field_start(record, field) else {
        return Vec::new();
    };
    let Some(end) = record.as_bytes()[start..]
        .iter()
        .position(|byte| *byte == b']')
    else {
        return Vec::new();
    };
    let array = &record[start..start + end + 1];
    let mut values = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = array[cursor..].find('"') {
        let quote = cursor + relative;
        let Some(end) = array.as_bytes()[quote + 1..]
            .iter()
            .position(|byte| *byte == b'"')
        else {
            break;
        };
        values.push(array[quote + 1..quote + end + 1].to_string());
        cursor = quote + end + 2;
    }
    values
}

fn js_number(record: &str, field: &str) -> Option<u64> {
    let start = field_start(record, field)?;
    let token: String = record.as_bytes()[start..]
        .iter()
        .take_while(|byte| matches!(**byte, b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-'))
        .map(|byte| char::from(*byte))
        .collect();
    token
        .parse::<f64>()
        .ok()
        .and_then(|value| value.is_finite().then_some(value as u64))
}

fn field_start(record: &str, field: &str) -> Option<usize> {
    let bytes = record.as_bytes();
    let mut cursor = 0;
    while let Some(relative) = record[cursor..].find(field) {
        let start = cursor + relative;
        let previous = start.checked_sub(1).and_then(|index| bytes.get(index));
        let after = start + field.len();
        let boundary = previous
            .is_none_or(|byte| *byte == b'{' || *byte == b',' || byte.is_ascii_whitespace());
        if boundary
            && bytes[after..]
                .iter()
                .position(|byte| !byte.is_ascii_whitespace())
                .and_then(|index| bytes.get(after + index))
                == Some(&b':')
        {
            let value_start = after
                + bytes[after..]
                    .iter()
                    .position(|byte| !byte.is_ascii_whitespace())
                    .unwrap_or(0)
                + 1;
            return bytes[value_start..]
                .iter()
                .position(|byte| !byte.is_ascii_whitespace())
                .map(|index| value_start + index);
        }
        cursor = after;
    }
    None
}

fn read_cache(runtime_dir: &Path, cli_version: &str) -> Option<Vec<SessionModel>> {
    cleanup_cache_temps(runtime_dir);
    let bytes = fs::read(runtime_dir.join(CACHE_FILE)).ok()?;
    let cache: CacheFile = serde_json::from_slice(&bytes).ok()?;
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_millis() as u64)?;
    (cache.derived
        && cache.fetched_at_ms > 0
        && cache.fetched_at_ms <= now_ms
        && cache.cli_version == cli_version
        && !cache.models.is_empty())
    .then_some(cache.models)
}

fn write_cache(runtime_dir: &Path, cli_version: &str, models: &[SessionModel]) -> bool {
    if fs::create_dir_all(runtime_dir).is_err() {
        eprintln!("could not create Claude model catalog cache directory");
        return false;
    }
    cleanup_cache_temps(runtime_dir);
    let fetched_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    let Ok(bytes) = serde_json::to_vec(&CacheFile {
        fetched_at_ms,
        cli_version: cli_version.to_string(),
        derived: true,
        models: models.to_vec(),
    }) else {
        eprintln!("could not encode Claude model catalog cache");
        return false;
    };
    let target = runtime_dir.join(CACHE_FILE);
    let temp = runtime_dir.join(format!(
        ".{CACHE_FILE}.tmp-{}-{}",
        std::process::id(),
        CACHE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(error) = fs::write(&temp, bytes) {
        eprintln!("could not write Claude model catalog cache: {error}");
        let _ = fs::remove_file(temp);
        return false;
    }
    if let Err(error) = fs::rename(&temp, &target) {
        let replaced = fs::remove_file(&target).is_ok_and(|()| fs::rename(&temp, &target).is_ok());
        if !replaced {
            eprintln!("could not replace Claude model catalog cache: {error}");
            let _ = fs::remove_file(temp);
            return false;
        }
    }
    true
}

fn cleanup_cache_temps(runtime_dir: &Path) {
    if let Ok(entries) = fs::read_dir(runtime_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&format!(".{CACHE_FILE}.tmp-")))
            {
                if let Err(error) = fs::remove_file(path) {
                    eprintln!("could not remove stale Claude model catalog temp file: {error}");
                }
            }
        }
    }
}

pub(crate) fn model_ids_match(left: &str, right: &str) -> bool {
    left.strip_suffix("[1m]").unwrap_or(left) == right.strip_suffix("[1m]").unwrap_or(right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::mpsc;

    #[derive(Clone)]
    struct BlockingSource {
        calls: Arc<AtomicU64>,
        release: Arc<AtomicBool>,
    }

    struct RetrySource {
        calls: Arc<AtomicU64>,
        first_done: mpsc::Sender<()>,
    }

    impl CatalogSource for BlockingSource {
        fn derive(&self) -> Result<Vec<SessionModel>, String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            while !self.release.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            Ok(fallback_models())
        }
    }

    impl CatalogSource for RetrySource {
        fn derive(&self) -> Result<Vec<SessionModel>, String> {
            if self.calls.fetch_add(1, Ordering::AcqRel) == 0 {
                self.first_done.send(()).expect("first attempt observed");
                return Err("temporary scrape failure".to_string());
            }
            Ok(fallback_models())
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "devboule-claude-catalog-{name}-{}-{}",
            std::process::id(),
            CACHE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temp dir");
        path
    }

    fn fake_binary(record: &str) -> PathBuf {
        let path = temp_dir("binary").join("claude.exe");
        let mut file = File::create(&path).expect("binary");
        file.write_all(&vec![b'x'; WINDOW_BYTES - 20])
            .expect("prefix");
        file.write_all(record.as_bytes()).expect("record");
        file.write_all(b"trailer").expect("trailer");
        path
    }

    #[test]
    fn derives_models_and_efforts_from_a_record_split_between_windows() {
        let path = fake_binary(
            r#"{id:"claude-opus-5",family:"opus",display_name:"Claude Opus 5",knowledge_cutoff:"2025-03",provider_ids:{first_party:"x"},context:{window:1e6},capabilities:["effort","xhigh_effort","max_effort"],default_effort:"xhigh"}"#,
        );
        let models = scrape_file(&path).expect("scrape");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model_id, "claude-opus-5");
        assert_eq!(models[0].name, "Claude Opus 5");
        assert_eq!(models[0].context_tokens, Some(1_000_000));
        assert_eq!(models[0].current_effort.as_deref(), Some("xhigh"));
        assert_eq!(
            models[0]
                .efforts
                .as_ref()
                .expect("efforts")
                .iter()
                .map(|effort| effort.id.as_str())
                .collect::<Vec<_>>(),
            ["low", "medium", "high", "xhigh"]
        );
        assert!(!models[0]
            .efforts
            .as_ref()
            .unwrap()
            .iter()
            .any(|effort| effort.id == "max"));
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn an_unbalanced_anchored_prefix_does_not_hide_following_models() {
        let path = temp_dir("false-prefix").join("claude.exe");
        let mut file = File::create(&path).expect("binary");
        file.write_all(&vec![b'x'; WINDOW_BYTES - 100])
            .expect("prefix");
        file.write_all(b"{id:\"claude-broken { never closes")
            .expect("false prefix");
        file.write_all(
            br#"{id:"claude-sonnet-5",family:"sonnet",display_name:"Claude Sonnet 5",capabilities:["effort"],default_effort:"high"}"#,
        )
        .expect("record");
        let models = scrape_file(&path).expect("scrape");
        assert_eq!(
            models
                .iter()
                .map(|model| model.model_id.as_str())
                .collect::<Vec<_>>(),
            ["claude-sonnet-5"]
        );
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn unchanged_version_reads_cache_without_deriving_again() {
        let dir = temp_dir("cache");
        let expected = fallback_models();
        let calls = std::cell::Cell::new(0);
        let first = load_with_deriver(&dir, "2.1.260", || {
            calls.set(calls.get() + 1);
            expected.clone()
        });
        memory_cache().lock().expect("memory cache").clear();
        let second = load_with_deriver(&dir, "2.1.260", || {
            calls.set(calls.get() + 1);
            Vec::new()
        });
        assert_eq!(first, second);
        assert_eq!(calls.get(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn changed_version_derives_again() {
        let dir = temp_dir("version");
        let calls = std::cell::Cell::new(0);
        let _ = load_with_deriver(&dir, "2.1.260", || {
            calls.set(calls.get() + 1);
            fallback_models()
        });
        memory_cache().lock().expect("memory cache").clear();
        let _ = load_with_deriver(&dir, "2.1.261", || {
            calls.set(calls.get() + 1);
            fallback_models()
        });
        assert_eq!(calls.get(), 2);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_scrape_explicitly_falls_back_to_aliases() {
        let dir = temp_dir("fallback");
        let models = load_with_deriver(&dir, "unknown", Vec::new);
        assert_eq!(
            models
                .iter()
                .map(|model| model.model_id.as_str())
                .collect::<Vec<_>>(),
            ["opus", "sonnet", "haiku"]
        );
        assert!(cached(&dir, "unknown").is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_derivation_is_not_cached_and_is_retried() {
        let dir = temp_dir("retry");
        let calls = Arc::new(AtomicU64::new(0));
        let (first_tx, first_rx) = mpsc::channel();
        let source = Arc::new(RetrySource {
            calls: Arc::clone(&calls),
            first_done: first_tx,
        });
        assert!(start_derivation(
            source.clone(),
            dir.clone(),
            "2.1.260".to_string(),
            |_| panic!("failed derivation must not publish a catalog"),
        ));
        first_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("first attempt completes");
        assert!(cached(&dir, "2.1.260").is_none());
        let retried = loop {
            if start_derivation(source.clone(), dir.clone(), "2.1.260".to_string(), |_| {}) {
                break true;
            }
            std::thread::yield_now();
        };
        assert!(retried);
        while cached(&dir, "2.1.260").is_none() {
            std::thread::yield_now();
        }
        assert_eq!(calls.load(Ordering::Acquire), 2);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_derivation_uses_one_in_flight_source() {
        let dir = temp_dir("dedupe");
        let calls = Arc::new(AtomicU64::new(0));
        let release = Arc::new(AtomicBool::new(false));
        let source = Arc::new(BlockingSource {
            calls: Arc::clone(&calls),
            release: Arc::clone(&release),
        });
        let (done_tx, done_rx) = mpsc::channel();
        assert!(start_derivation(
            source.clone(),
            dir.clone(),
            "2.1.260".to_string(),
            move |_| done_tx.send(()).expect("derivation callback"),
        ));
        while calls.load(Ordering::Acquire) == 0 {
            std::thread::yield_now();
        }
        assert!(!start_derivation(
            source,
            dir.clone(),
            "2.1.260".to_string(),
            |_| panic!("duplicate derivation callback"),
        ));
        release.store(true, Ordering::Release);
        done_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("derivation completes");
        assert_eq!(calls.load(Ordering::Acquire), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn manifest_is_stored_before_any_cli_init_frame() {
        let runtime = crate::session::SessionRuntime::with_journal("claude-test".to_string(), None);
        let stored = runtime.store_session_manifest(initial_manifest(fallback_models()));
        let SessionEvent::SessionManifest { models, .. } = stored else {
            panic!("initial Claude manifest must be a session manifest");
        };
        assert_eq!(models.len(), 3);
        assert_eq!(runtime.session_manifest(), Some(initial_manifest(models)));
    }

    #[test]
    fn system_init_reconciles_current_model_without_dropping_catalog() {
        let runtime = crate::session::SessionRuntime::with_journal("claude-test".to_string(), None);
        let initial = SessionEvent::SessionManifest {
            provider_id: Some("claude".to_string()),
            current_model_id: Some("claude-sonnet-5".to_string()),
            models: vec![
                SessionModel {
                    model_id: "claude-sonnet-5".to_string(),
                    name: "Claude Sonnet 5".to_string(),
                    description: None,
                    context_tokens: None,
                    current_effort: Some("high".to_string()),
                    efforts: Some(efforts(false, "high")),
                },
                SessionModel {
                    model_id: "claude-opus-5".to_string(),
                    name: "Claude Opus 5".to_string(),
                    description: None,
                    context_tokens: None,
                    current_effort: Some("xhigh".to_string()),
                    efforts: Some(efforts(true, "xhigh")),
                },
            ],
            modes: None,
        };
        runtime.store_session_manifest(initial);
        let reconciled = runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("claude".to_string()),
            current_model_id: Some("claude-opus-5".to_string()),
            models: vec![SessionModel {
                model_id: "claude-opus-5".to_string(),
                name: "claude-opus-5".to_string(),
                description: None,
                context_tokens: None,
                current_effort: None,
                efforts: None,
            }],
            modes: None,
        });
        let SessionEvent::SessionManifest { models, .. } = reconciled else {
            panic!("reconciled event must be a manifest");
        };
        assert_eq!(models.len(), 2);
        assert_eq!(models[1].name, "Claude Opus 5");
        assert_eq!(models[1].efforts.as_ref().unwrap().len(), 4);
    }

    #[test]
    fn background_catalog_replaces_fallback_without_losing_current_variant() {
        let runtime = crate::session::SessionRuntime::with_journal("claude-test".to_string(), None);
        runtime.store_session_manifest(initial_manifest(fallback_models()));
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("claude".to_string()),
            current_model_id: Some("claude-opus-5[1m]".to_string()),
            models: vec![SessionModel {
                model_id: "claude-opus-5[1m]".to_string(),
                name: "claude-opus-5[1m]".to_string(),
                description: None,
                context_tokens: None,
                current_effort: None,
                efforts: None,
            }],
            modes: None,
        });
        let stored = runtime.store_claude_catalog(manifest_with_current(
            vec![SessionModel {
                model_id: "claude-opus-5".to_string(),
                name: "Claude Opus 5".to_string(),
                description: None,
                context_tokens: None,
                current_effort: Some("xhigh".to_string()),
                efforts: Some(efforts(true, "xhigh")),
            }],
            Some("claude-opus-5[1m]".to_string()),
        ));
        let SessionEvent::SessionManifest {
            current_model_id,
            models,
            ..
        } = stored
        else {
            panic!("catalog update must remain a manifest");
        };
        assert_eq!(current_model_id.as_deref(), Some("claude-opus-5[1m]"));
        assert_eq!(models.len(), 2);
        assert!(models
            .iter()
            .all(|model| !["opus", "sonnet", "haiku"].contains(&model.model_id.as_str())));
        assert!(models
            .iter()
            .any(|model| model.model_id == "claude-opus-5[1m]"));
    }

    #[test]
    fn observed_current_model_is_kept_when_derivation_does_not_contain_it() {
        let runtime = crate::session::SessionRuntime::with_journal("claude-test".to_string(), None);
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("claude".to_string()),
            current_model_id: Some("claude-opus-4-6".to_string()),
            models: Vec::new(),
            modes: None,
        });
        let stored = runtime.store_claude_catalog(manifest_with_current(
            vec![SessionModel {
                model_id: "claude-sonnet-5".to_string(),
                name: "Claude Sonnet 5".to_string(),
                description: None,
                context_tokens: None,
                current_effort: Some("high".to_string()),
                efforts: Some(efforts(false, "high")),
            }],
            Some("claude-opus-4-6".to_string()),
        ));
        let SessionEvent::SessionManifest {
            current_model_id,
            models,
            ..
        } = stored
        else {
            panic!("catalog update must remain a manifest");
        };
        assert_eq!(current_model_id.as_deref(), Some("claude-opus-4-6"));
        assert!(models
            .iter()
            .any(|model| model.model_id == "claude-opus-4-6"));
    }

    fn load_with_deriver<F>(dir: &Path, version: &str, derive: F) -> Vec<SessionModel>
    where
        F: FnOnce() -> Vec<SessionModel>,
    {
        let key = (dir.to_path_buf(), version.to_string());
        if let Some(models) = memory_cache()
            .lock()
            .expect("memory cache")
            .get(&key)
            .cloned()
        {
            return models;
        }
        let models = read_cache(dir, version);
        let models = if let Some(models) = models {
            models
        } else {
            let models = derive();
            if models.is_empty() {
                return fallback_models();
            }
            let _ = write_cache(dir, version, &models);
            memory_cache()
                .lock()
                .expect("memory cache")
                .insert(key, models.clone());
            models
        };
        models
    }
}
