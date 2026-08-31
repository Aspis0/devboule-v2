//! Finding the installed plugins and deciding which of them the host will run.
//!
//! Discovery is a scan of `<app data>/plugins/`. Each subdirectory is a
//! candidate; a candidate becomes usable only if its manifest parses, its file
//! list matches the directory exactly, and every digest agrees.
//!
//! ## A directory that cannot be read is not an empty directory
//!
//! This distinction has cost this project real damage once already: a collector
//! that reported an unreadable folder as an empty list, wired to something that
//! deleted what was missing. Nothing here deletes, but the reporting rule is the
//! same and is applied at both levels. A plugins root that is *absent* means no
//! plugins are installed, which is the state the app ships in. A plugins root
//! that *exists and cannot be read* is a problem with a sentence attached, and a
//! plugin directory that cannot be read is a refused plugin, not an absent one.
//! Silence would tell the user their plugin was never installed.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::assets::{safe_relative_segments, MAX_ASSET_BYTES};
use super::manifest::{
    parse_manifest, PluginManifest, MANIFEST_FILE_NAME, MAX_MANIFEST_BYTES, MAX_PLUGIN_FILES,
};

/// A plugin whose files add up to more than this is refused before a single
/// byte is hashed. Verification reads everything it verifies, so without a
/// ceiling a mistaken install — a virtual-machine image dropped in the
/// directory — is an unbounded read on the path that opens a surface.
pub(super) const MAX_PLUGIN_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// A symlink, and on Windows any NTFS reparse point — junctions included.
/// `FileType::is_symlink()` is only `IO_REPARSE_TAG_SYMLINK`.
fn is_link(path: &Path, file_type: &std::fs::FileType) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0,
            Err(_) => file_type.is_symlink(),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        file_type.is_symlink()
    }
}

/// One candidate directory and what came of it.
pub struct ScannedPlugin {
    /// The directory name, which is also the id the WebView's URLs carry.
    pub id: String,
    pub outcome: Result<PluginManifest, String>,
}

/// The result of one scan. Both the readout the surface shows and the lookup the
/// asset server uses are derived from this, so the two cannot drift apart: a
/// plugin the user is told was refused is a plugin whose files will not load.
pub struct Scan {
    pub root: PathBuf,
    /// Set when the plugins root exists but could not be listed.
    pub problem: Option<String>,
    pub plugins: Vec<ScannedPlugin>,
}

impl Scan {
    /// The verified manifest for a plugin, or nothing if it was refused.
    pub fn ready(&self, id: &str) -> Option<&PluginManifest> {
        self.plugins
            .iter()
            .find(|plugin| plugin.id == id)
            .and_then(|plugin| plugin.outcome.as_ref().ok())
    }

    pub fn inventory(&self) -> PluginInventory {
        PluginInventory {
            root: self.root.display().to_string(),
            problem: self.problem.clone(),
            plugins: self
                .plugins
                .iter()
                .map(|plugin| match &plugin.outcome {
                    Ok(manifest) => PluginEntry {
                        id: plugin.id.clone(),
                        name: Some(manifest.name.clone()),
                        version: Some(manifest.version.clone()),
                        capabilities: manifest.capabilities.clone(),
                        ui_entry: Some(manifest.ui_entry.clone()),
                        ready: true,
                        reason: None,
                    },
                    Err(reason) => PluginEntry {
                        id: plugin.id.clone(),
                        name: None,
                        version: None,
                        capabilities: Vec::new(),
                        ui_entry: None,
                        ready: false,
                        reason: Some(reason.clone()),
                    },
                })
                .collect(),
        }
    }
}

/// What the surface is told. Deliberately flat and complete: a refused plugin
/// appears here with its reason rather than being left out, because "your
/// plugin is not installed" is the wrong thing to say to someone who installed
/// it and got one file wrong.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInventory {
    pub root: String,
    pub plugins: Vec<PluginEntry>,
    pub problem: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginEntry {
    pub id: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub capabilities: Vec<String>,
    /// Path of the HTML document the host frames. Absent when the plugin was
    /// refused, so the surface can hide a broken tile rather than pointing the
    /// iframe at a 404.
    pub ui_entry: Option<String>,
    pub ready: bool,
    pub reason: Option<String>,
}

/// Scan the plugins root once.
pub fn scan(root: &Path) -> Scan {
    // A crash between the two install renames leaves the working copy in
    // staging. Repair is a named function so this one stays a read of what
    // is installed after the disk is honest again.
    super::install::restore_interrupted_swaps(root);

    let mut scanned = Scan {
        root: root.to_path_buf(),
        problem: None,
        plugins: Vec::new(),
    };

    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        // Absent is the shipping state, and says nothing.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return scanned,
        Err(error) => {
            scanned.problem = Some(format!(
                "{} exists but could not be listed ({error}), so any plugin inside it is \
                 unaccounted for rather than absent",
                root.display()
            ));
            return scanned;
        }
    };

    let mut candidates: Vec<(String, PathBuf, bool)> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                scanned.problem = Some(format!(
                    "{} could not be listed to the end ({error}), so this list may be short",
                    root.display()
                ));
                break;
            }
        };
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if is_link(&entry.path(), &file_type) {
            // A link would put the verified directory somewhere the asset
            // server refuses to serve from, so the plugin would verify and then
            // silently fail to load. Refusing here says why.
            candidates.push((name, entry.path(), true));
            continue;
        }
        if file_type.is_dir() {
            candidates.push((name, entry.path(), false));
        }
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));

    scanned.plugins = candidates
        .into_iter()
        .map(|(id, path, is_link)| ScannedPlugin {
            outcome: if is_link {
                Err(format!(
                    "{id} is a link rather than a directory, and a plugin has to be the files \
                     themselves"
                ))
            } else {
                verify(&path, &id)
            },
            id,
        })
        .collect();
    scanned
}

/// Read one plugin directory and decide whether it may run.
pub fn verify(directory: &Path, id: &str) -> Result<PluginManifest, String> {
    let manifest_path = directory.join(MANIFEST_FILE_NAME);
    let metadata = std::fs::metadata(&manifest_path)
        .map_err(|error| format!("{MANIFEST_FILE_NAME} could not be read: {error}"))?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(format!(
            "{MANIFEST_FILE_NAME} is {} bytes, which is not a manifest",
            metadata.len()
        ));
    }
    let bytes = std::fs::read(&manifest_path)
        .map_err(|error| format!("{MANIFEST_FILE_NAME} could not be read: {error}"))?;
    let manifest = parse_manifest(&bytes, id)?;

    let present = list_files(directory)?;
    let listed: BTreeSet<&String> = manifest.files.keys().collect();
    let found: BTreeSet<&String> = present.keys().collect();

    if let Some(missing) = listed.difference(&found).next() {
        return Err(format!(
            "{MANIFEST_FILE_NAME} lists {missing}, which is not there"
        ));
    }
    if let Some(extra) = found.difference(&listed).next() {
        // The manifest describes the directory exactly; see the module note on
        // why an unlisted file is refused rather than ignored.
        return Err(format!(
            "{extra} is in the plugin but not in {MANIFEST_FILE_NAME}, so nothing vouches for it"
        ));
    }

    let total: u64 = present.values().sum();
    if total > MAX_PLUGIN_BYTES {
        return Err(format!(
            "the plugin holds {total} bytes, more than the {MAX_PLUGIN_BYTES} this build will read \
             to verify it"
        ));
    }

    for (relative, size) in &present {
        if *size > MAX_ASSET_BYTES && manifest.backend_entry.as_deref() != Some(relative.as_str()) {
            return Err(format!("{relative} is too large to be served"));
        }
    }

    for (relative, expected) in &manifest.files {
        let digest = sha256_file(&directory.join(relative))
            .map_err(|error| format!("{relative} could not be read to verify it: {error}"))?;
        if &digest != expected {
            return Err(format!(
                "{relative} is not the file {MANIFEST_FILE_NAME} describes"
            ));
        }
    }

    Ok(manifest)
}

/// Every file under `directory`, by normalised relative path, with its size.
///
/// The top-level manifest is left out: it is the thing doing the describing and
/// cannot carry its own digest.
pub(super) fn list_files(directory: &Path) -> Result<BTreeMap<String, u64>, String> {
    let mut files = BTreeMap::new();
    let mut pending: Vec<(PathBuf, String)> = vec![(directory.to_path_buf(), String::new())];

    while let Some((current, prefix)) = pending.pop() {
        let entries = std::fs::read_dir(&current).map_err(|error| {
            format!(
                "{} could not be listed ({error}), so the plugin cannot be accounted for",
                current.display()
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "{} could not be listed to the end: {error}",
                    current.display()
                )
            })?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                return Err(format!(
                    "{} holds a file whose name is not text, and a manifest cannot name it",
                    current.display()
                ));
            };
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            // The same grammar the asset server resolves a request with: a file
            // it could never address has no business being installed.
            let Some(relative) = safe_relative_segments(&relative) else {
                return Err(format!(
                    "{relative} cannot be addressed by the plugin server and must be renamed"
                ));
            };

            let file_type = entry
                .file_type()
                .map_err(|error| format!("{relative} could not be examined: {error}"))?;
            if is_link(&entry.path(), &file_type) {
                return Err(format!(
                    "{relative} is a link, and a plugin has to be the files themselves"
                ));
            }
            if file_type.is_dir() {
                pending.push((entry.path(), relative));
                continue;
            }
            if !file_type.is_file() {
                return Err(format!("{relative} is neither a file nor a directory"));
            }
            if relative == MANIFEST_FILE_NAME {
                continue;
            }
            if files.len() >= MAX_PLUGIN_FILES {
                return Err(format!(
                    "the plugin holds more than {MAX_PLUGIN_FILES} files, more than this build \
                     will verify"
                ));
            }
            let size = entry
                .metadata()
                .map_err(|error| format!("{relative} could not be measured: {error}"))?
                .len();
            files.insert(relative, size);
        }
    }
    Ok(files)
}

/// Hash a file without holding it in memory: a plugin's executable is
/// legitimately large, and verification must not scale with it.
fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
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

#[cfg(test)]
mod tests {
    use super::*;

    fn digest_of(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Write a plugin whose manifest is correct for the files written with it.
    fn install(root: &Path, id: &str, files: &[(&str, &[u8])]) -> PathBuf {
        let directory = root.join(id);
        let mut listed = serde_json::Map::new();
        for (relative, contents) in files {
            let path = directory.join(relative);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            std::fs::write(&path, contents).expect("write");
            listed.insert(
                (*relative).to_string(),
                serde_json::Value::String(digest_of(contents)),
            );
        }
        let manifest = serde_json::json!({
            "manifestVersion": 1,
            "id": id,
            "name": "Polis",
            "version": "0.1.0",
            "entry": { "ui": "ui/index.html" },
            "capabilities": ["oracle.search"],
            "files": serde_json::Value::Object(listed),
        });
        std::fs::write(
            directory.join(MANIFEST_FILE_NAME),
            serde_json::to_vec_pretty(&manifest).expect("json"),
        )
        .expect("write manifest");
        directory
    }

    const UI: &[u8] = b"export const surface = 1;\n";

    #[test]
    fn a_correctly_installed_plugin_is_ready() {
        let temp = tempfile::tempdir().expect("tempdir");
        install(temp.path(), "polis", &[("ui/index.html", UI)]);
        let scan = scan(temp.path());
        assert_eq!(scan.problem, None);
        assert_eq!(scan.plugins.len(), 1);
        let manifest = scan.ready("polis").expect("polis should be ready");
        assert_eq!(manifest.ui_entry, "ui/index.html");
        let inventory = scan.inventory();
        assert!(inventory.plugins[0].ready);
        assert_eq!(inventory.plugins[0].reason, None);
        assert_eq!(inventory.plugins[0].capabilities, vec!["oracle.search"]);
    }

    #[test]
    fn a_ready_plugin_serializes_its_document_path_as_ui_entry() {
        let temp = tempfile::tempdir().expect("tempdir");
        install(temp.path(), "polis", &[("ui/index.html", UI)]);
        let json = serde_json::to_value(&scan(temp.path()).inventory().plugins[0]).expect("json");
        assert_eq!(
            json.get("uiEntry").and_then(|value| value.as_str()),
            Some("ui/index.html"),
            "the surface reads camelCase uiEntry: {json}"
        );
    }

    #[test]
    fn a_refused_plugin_serializes_ui_entry_as_null() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("polis")).expect("mkdir");
        let json = serde_json::to_value(&scan(temp.path()).inventory().plugins[0]).expect("json");
        assert_eq!(
            json.get("uiEntry"),
            Some(&serde_json::Value::Null),
            "omitting the key is not the null the surface handles: {json}"
        );
    }

    #[test]
    fn one_changed_byte_is_enough() {
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = install(temp.path(), "polis", &[("ui/index.html", UI)]);
        std::fs::write(
            directory.join("ui/index.html"),
            b"export const surface = 2;\n",
        )
        .expect("tamper");
        let scan = scan(temp.path());
        assert!(scan.ready("polis").is_none());
        let reason = scan.inventory().plugins[0]
            .reason
            .clone()
            .expect("a refusal has a reason");
        assert!(
            reason.contains("ui/index.html") && reason.contains("describes"),
            "the reason should name the file that changed: {reason}"
        );
    }

    #[test]
    fn a_file_nobody_listed_is_refused_rather_than_ignored() {
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = install(temp.path(), "polis", &[("ui/index.html", UI)]);
        // The shape this rule exists for: a library the manifest never mentions,
        // sitting where the plugin's own process would find it first.
        std::fs::write(directory.join("version.dll"), b"MZ").expect("write");
        let scan = scan(temp.path());
        assert!(scan.ready("polis").is_none());
        let reason = scan.inventory().plugins[0].reason.clone().expect("reason");
        assert!(
            reason.contains("version.dll"),
            "the reason should name the unlisted file: {reason}"
        );
    }

    #[test]
    fn a_missing_file_is_refused_and_named() {
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = install(
            temp.path(),
            "polis",
            &[("ui/index.html", UI), ("data.bin", b"x")],
        );
        std::fs::remove_file(directory.join("data.bin")).expect("remove");
        let scan = scan(temp.path());
        let reason = scan.inventory().plugins[0].reason.clone().expect("reason");
        assert!(
            reason.contains("data.bin") && reason.contains("not there"),
            "the reason should name the missing file: {reason}"
        );
    }

    #[test]
    fn a_plugin_in_the_wrong_directory_is_refused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = install(temp.path(), "polis", &[("ui/index.html", UI)]);
        std::fs::rename(&directory, temp.path().join("oracle")).expect("rename");
        let scan = scan(temp.path());
        let reason = scan.inventory().plugins[0].reason.clone().expect("reason");
        assert!(
            reason.contains("installed in"),
            "renaming the directory must not let a plugin answer for another: {reason}"
        );
    }

    #[test]
    fn a_missing_plugins_root_says_nothing_but_an_unreadable_one_speaks_up() {
        let temp = tempfile::tempdir().expect("tempdir");
        let absent = scan(&temp.path().join("plugins"));
        assert_eq!(absent.problem, None, "not installed is not a problem");
        assert!(absent.plugins.is_empty());

        // A file where the directory should be is the readable stand-in for the
        // case this rule is really about — an unmounted drive, a folder held by
        // something else. Both arrive as a `read_dir` error that is not
        // `NotFound`, and both must not be reported as "no plugins installed".
        let impostor = temp.path().join("not-a-directory");
        std::fs::write(&impostor, b"").expect("write");
        let broken = scan(&impostor);
        assert!(
            broken.problem.is_some(),
            "an unreadable plugins root reported as empty is the bug this test exists for"
        );
        assert!(broken.plugins.is_empty());
    }

    #[test]
    fn a_plugin_directory_that_cannot_be_read_is_refused_not_skipped() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("polis")).expect("mkdir");
        // No manifest at all: the same shape as a directory we cannot read into.
        let scan = scan(temp.path());
        assert_eq!(scan.plugins.len(), 1, "the candidate must still be listed");
        let entry = &scan.inventory().plugins[0];
        assert!(!entry.ready);
        assert!(
            entry
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("plugin.json"),
            "the user has to be told why, not just that nothing appeared"
        );
    }

    #[test]
    fn a_linked_plugin_directory_is_refused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let elsewhere = temp.path().join("elsewhere");
        install(&elsewhere, "polis", &[("ui/index.html", UI)]);
        let root = temp.path().join("plugins");
        std::fs::create_dir_all(&root).expect("mkdir");

        #[cfg(windows)]
        let linked =
            std::os::windows::fs::symlink_dir(elsewhere.join("polis"), root.join("polis")).is_ok();
        #[cfg(unix)]
        let linked =
            std::os::unix::fs::symlink(elsewhere.join("polis"), root.join("polis")).is_ok();
        if !linked {
            eprintln!("skipped the linked-directory case: this machine would not create one");
            return;
        }
        let scan = scan(&root);
        assert!(scan.ready("polis").is_none());
        let reason = scan.inventory().plugins[0].reason.clone().expect("reason");
        assert!(reason.contains("link"), "wrong reason: {reason}");
    }

    #[test]
    fn a_linked_file_inside_a_plugin_is_refused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let secret = temp.path().join("private.txt");
        std::fs::write(&secret, b"a secret the app can read").expect("write");
        let directory = install(temp.path(), "polis", &[("ui/index.html", UI)]);

        #[cfg(windows)]
        let linked =
            std::os::windows::fs::symlink_file(&secret, directory.join("link.txt")).is_ok();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&secret, directory.join("link.txt")).is_ok();
        if !linked {
            eprintln!("skipped the linked-file case: this machine would not create one");
            return;
        }
        let scan = scan(temp.path());
        let reason = scan.inventory().plugins[0].reason.clone().expect("reason");
        assert!(reason.contains("link"), "wrong reason: {reason}");
    }

    /// NTFS junctions do not need privilege, unlike symlinks. `mklink /J` is the
    /// way a machine that will not create a symlink still reproduces the hole.
    #[cfg(windows)]
    fn try_junction(link: &Path, target: &Path) -> bool {
        std::process::Command::new("cmd")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_inside_a_plugin_is_refused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let elsewhere = temp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("mkdir");
        std::fs::write(elsewhere.join("secret.txt"), b"not the plugin").expect("write");
        // A dedicated plugins root, so the junction target is not itself a
        // candidate sitting next to the plugin.
        let root = temp.path().join("plugins");
        let directory = install(&root, "polis", &[("ui/index.html", UI)]);
        let escape = directory.join("escape");
        if !try_junction(&escape, &elsewhere) {
            eprintln!("skipped the junction-inside-plugin case: mklink /J failed");
            return;
        }
        let scan = scan(&root);
        let reason = scan
            .plugins
            .iter()
            .find(|plugin| plugin.id == "polis")
            .expect("polis listed")
            .outcome
            .as_ref()
            .expect_err("a junction inside the plugin must refuse it");
        assert!(
            reason.contains("link"),
            "a junction walked as a directory is the hole this test exists for: {reason}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_used_as_the_plugin_directory_is_refused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let elsewhere = temp.path().join("elsewhere");
        install(&elsewhere, "polis", &[("ui/index.html", UI)]);
        let root = temp.path().join("plugins");
        std::fs::create_dir_all(&root).expect("mkdir");
        let linked = root.join("polis");
        if !try_junction(&linked, &elsewhere.join("polis")) {
            eprintln!("skipped the junction-as-plugin-directory case: mklink /J failed");
            return;
        }
        let scan = scan(&root);
        assert!(
            scan.ready("polis").is_none(),
            "a junction used as the plugin directory verified files that live elsewhere"
        );
        let reason = scan
            .plugins
            .iter()
            .find(|plugin| plugin.id == "polis")
            .expect("polis listed")
            .outcome
            .as_ref()
            .expect_err("must be refused");
        assert!(reason.contains("link"), "wrong reason: {reason}");
    }

    #[test]
    fn the_digest_is_the_one_the_rest_of_the_world_computes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("a.bin");
        std::fs::write(&path, b"abc").expect("write");
        assert_eq!(
            sha256_file(&path).expect("hash"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_file_larger_than_the_buffer_hashes_the_same_as_all_of_it() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("big.bin");
        // Deliberately not a multiple of the 64 KiB read buffer, so a bug that
        // dropped or repeated the tail would show.
        let contents: Vec<u8> = (0..200_000_u32).map(|index| index as u8).collect();
        std::fs::write(&path, &contents).expect("write");
        assert_eq!(sha256_file(&path).expect("hash"), digest_of(&contents));
    }

    /// A sparse file of `bytes` length. `set_len` is enough: we need the size,
    /// not 64 MiB of real zeroes on disk.
    fn sparse_file(path: &Path, bytes: u64) -> String {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        let file = std::fs::File::create(path).expect("create");
        file.set_len(bytes).expect("set_len");
        sha256_file(path).expect("hash")
    }

    #[test]
    fn a_ui_file_larger_than_the_servable_ceiling_is_refused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = temp.path().join("polis");
        std::fs::create_dir_all(directory.join("ui")).expect("mkdir");
        std::fs::write(directory.join("ui/index.html"), UI).expect("write");
        let atlas_digest = sparse_file(&directory.join("ui/atlas.png"), super::MAX_ASSET_BYTES + 1);
        let manifest = serde_json::json!({
            "manifestVersion": 1,
            "id": "polis",
            "name": "Polis",
            "version": "0.1.0",
            "entry": { "ui": "ui/index.html" },
            "files": {
                "ui/index.html": digest_of(UI),
                "ui/atlas.png": atlas_digest,
            },
        });
        std::fs::write(
            directory.join(MANIFEST_FILE_NAME),
            serde_json::to_vec(&manifest).expect("json"),
        )
        .expect("write");

        let scan = scan(temp.path());
        assert!(
            scan.ready("polis").is_none(),
            "an asset too large to serve must not verify as ready"
        );
        let reason = scan.inventory().plugins[0].reason.clone().expect("reason");
        assert!(
            reason.contains("ui/atlas.png") && reason.contains("too large to be served"),
            "the reason should name the file and say it cannot be served: {reason}"
        );
    }

    #[test]
    fn a_backend_entry_larger_than_the_servable_ceiling_is_still_verified() {
        let temp = tempfile::tempdir().expect("tempdir");
        let directory = temp.path().join("polis");
        std::fs::create_dir_all(directory.join("ui")).expect("mkdir");
        std::fs::write(directory.join("ui/index.html"), UI).expect("write");
        let backend_digest = sparse_file(
            &directory.join("polis-backend.exe"),
            super::MAX_ASSET_BYTES + 1,
        );
        let manifest = serde_json::json!({
            "manifestVersion": 1,
            "id": "polis",
            "name": "Polis",
            "version": "0.1.0",
            "entry": { "ui": "ui/index.html", "backend": "polis-backend.exe" },
            "files": {
                "ui/index.html": digest_of(UI),
                "polis-backend.exe": backend_digest,
            },
        });
        std::fs::write(
            directory.join(MANIFEST_FILE_NAME),
            serde_json::to_vec(&manifest).expect("json"),
        )
        .expect("write");

        let scan = scan(temp.path());
        assert!(
            scan.ready("polis").is_some(),
            "the backend is executed as a process and never served, so its size is not the asset ceiling"
        );
    }
}
