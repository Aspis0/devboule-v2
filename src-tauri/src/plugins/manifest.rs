//! What a plugin says it is, and what the host insists on before believing it.
//!
//! A plugin is a directory under `<app data>/plugins/` containing a
//! `plugin.json`. That file is written by whoever built the plugin, so nothing
//! in it is trusted: every field is bounded, every path is re-derived with the
//! same rules the asset server uses, and anything unrecognised is a refusal
//! rather than a shrug.
//!
//! ## Why the file list is exhaustive
//!
//! `files` must name **every** file in the plugin directory, not just the ones
//! the manifest cares about. That is stricter than it first looks and it is
//! deliberate: a checksum on the executable alone is worth very little on
//! Windows, where dropping an unlisted DLL beside it is enough to get code
//! loaded into the plugin process. Making the manifest an exact description of
//! the directory closes that, and it also catches the ordinary case — a file
//! left behind by a previous version of the plugin.
//!
//! The consequence is that **a plugin must not write into its own directory**.
//! Its code and assets live there and are read-only; anything it saves belongs
//! in a per-plugin data directory the host hands it at spawn time. Polis has a
//! city to save, so this is not hypothetical.
//!
//! ## What the checksums are actually worth
//!
//! They prove *integrity*, not *authenticity*. They catch a truncated download,
//! a half-finished install, a corrupted file, a stale leftover, and an unlisted
//! file dropped into the directory. They do **not** stop someone who can write
//! into that directory, because that someone can rewrite `plugin.json` too.
//!
//! This is worth stating plainly because the design note originally claimed the
//! checksums were what made the content-policy widening safe. They are not.
//! They make the widening *deliberate* — the app executes the bytes it was told
//! about, and notices when they change underneath it. Authenticity needs a
//! signature over the manifest verified with a key that does not live in the
//! plugin directory, and that needs a publishing side that does not exist yet.

use std::collections::BTreeMap;

use super::assets::safe_relative_segments;

/// The only manifest version this build understands.
pub const SUPPORTED_MANIFEST_VERSION: u32 = 1;

/// The manifest file inside a plugin directory.
pub const MANIFEST_FILE_NAME: &str = "plugin.json";

/// A manifest larger than this is not a manifest.
pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// Bound on the plugin id, which also has to be a usable directory name.
const MAX_ID_LEN: usize = 64;

/// Bound on the free text fields, which are shown to a person.
const MAX_TEXT_LEN: usize = 128;

/// A plugin with more files than this is not something we are going to verify.
pub const MAX_PLUGIN_FILES: usize = 10_000;

/// Requested capabilities are an open set, but not an unbounded one.
const MAX_CAPABILITIES: usize = 64;

/// A manifest that parsed and passed every check.
///
/// Paths are normalised: exactly the form [`safe_relative_segments`] produces,
/// which is exactly the form the asset server resolves a request to. Hashes are
/// lowercase hex, 64 characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub ui_entry: String,
    pub backend_entry: Option<String>,
    /// Requested, not granted. The host decides what it advertises at handshake
    /// time; an unknown name here is not an error, it simply gets nothing.
    pub capabilities: Vec<String>,
    /// Every file in the directory, normalised path to lowercase hex digest.
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawManifest {
    manifest_version: u32,
    id: String,
    name: String,
    version: String,
    entry: RawEntry,
    #[serde(default)]
    capabilities: Vec<String>,
    files: BTreeMap<String, String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawEntry {
    ui: String,
    #[serde(default)]
    backend: Option<String>,
}

/// Parse and check a manifest.
///
/// `directory_name` is the folder the file was found in. The id has to match
/// it: the URL the WebView asks for carries the directory name, so a manifest
/// claiming a different id would let one plugin answer for another's assets.
///
/// The error is a sentence for a person, because that is what every caller does
/// with it — the surface shows a plugin as present-but-refused, with the reason.
/// A typed error would be re-rendered into exactly this string at the boundary.
pub fn parse_manifest(bytes: &[u8], directory_name: &str) -> Result<PluginManifest, String> {
    let raw: RawManifest = serde_json::from_slice(bytes)
        .map_err(|error| format!("{MANIFEST_FILE_NAME} is not valid: {error}"))?;

    if raw.manifest_version != SUPPORTED_MANIFEST_VERSION {
        return Err(format!(
            "manifestVersion is {}, and this build of Devboule understands {}",
            raw.manifest_version, SUPPORTED_MANIFEST_VERSION
        ));
    }

    check_id(&raw.id)?;
    if raw.id != directory_name {
        return Err(format!(
            "the manifest calls this plugin {} but it is installed in {}",
            quote(&raw.id),
            quote(directory_name)
        ));
    }
    let name = check_text("name", &raw.name)?;
    let version = check_text("version", &raw.version)?;

    if raw.files.len() > MAX_PLUGIN_FILES {
        return Err(format!(
            "files lists {} entries, more than the {MAX_PLUGIN_FILES} this build will verify",
            raw.files.len()
        ));
    }
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    for (path, hash) in &raw.files {
        let normalised = safe_relative_segments(path)
            .ok_or_else(|| format!("files names a path outside the plugin: {}", quote(path)))?;
        if normalised == MANIFEST_FILE_NAME {
            return Err(format!(
                "files lists {MANIFEST_FILE_NAME}, which cannot describe its own digest"
            ));
        }
        let digest = check_digest(&normalised, hash)?;
        if files.insert(normalised.clone(), digest).is_some() {
            return Err(format!(
                "two entries in files describe the same file: {}",
                quote(&normalised)
            ));
        }
    }
    if files.is_empty() {
        return Err("files is empty, so there is nothing to verify".to_string());
    }

    let ui_entry = check_entry("entry.ui", &raw.entry.ui, &files)?;
    if !(ui_entry.ends_with(".html") || ui_entry.ends_with(".htm")) {
        // The host loads this in a cross-origin iframe as a document. A module
        // entry would be fetched as a script and the frame would render nothing.
        return Err(format!(
            "entry.ui is loaded as a document and must end in .html or .htm: {}",
            quote(&ui_entry)
        ));
    }
    let backend_entry = match &raw.entry.backend {
        Some(backend) => Some(check_entry("entry.backend", backend, &files)?),
        None => None,
    };

    if raw.capabilities.len() > MAX_CAPABILITIES {
        return Err(format!(
            "capabilities lists {} names, more than the {MAX_CAPABILITIES} allowed",
            raw.capabilities.len()
        ));
    }
    let mut capabilities: Vec<String> = Vec::with_capacity(raw.capabilities.len());
    for capability in &raw.capabilities {
        let checked = check_text("a capability", capability)?;
        if !capabilities.contains(&checked) {
            capabilities.push(checked);
        }
    }

    Ok(PluginManifest {
        id: raw.id,
        name,
        version,
        ui_entry,
        backend_entry,
        capabilities,
        files,
    })
}

/// The id is a directory name, a URL path segment and a routing key at once, so
/// it is held to the narrowest form that works as all three.
pub fn check_id(id: &str) -> Result<(), String> {
    let usable = !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-');
    if usable {
        Ok(())
    } else {
        Err(format!(
            "id must be 1 to {MAX_ID_LEN} characters of lowercase letters, digits and dashes: {}",
            quote(id)
        ))
    }
}

/// Free text that reaches a readout. Control characters are refused rather than
/// stripped: a name carrying a newline or a terminal escape is either a broken
/// generator or an attempt to forge a line of the interface, and neither should
/// be quietly cleaned up and shown.
fn check_text(field: &str, value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} is empty"));
    }
    if trimmed.chars().count() > MAX_TEXT_LEN {
        return Err(format!("{field} is longer than {MAX_TEXT_LEN} characters"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(format!("{field} contains a control character"));
    }
    Ok(trimmed.to_string())
}

/// A digest is 64 hex characters. Uppercase is accepted and folded down, for a
/// concrete reason: `sha256sum` writes lowercase and PowerShell's `Get-FileHash`
/// writes uppercase, and refusing one of them would fail every manifest a
/// Windows author produced with the tool that is already on their machine.
fn check_digest(path: &str, hash: &str) -> Result<String, String> {
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "the digest for {} is not 64 hexadecimal characters: {}",
            quote(path),
            quote(hash)
        ));
    }
    Ok(hash.to_ascii_lowercase())
}

/// An entry point has to be a safe path *and* be listed in `files`. Without the
/// second half the one file that certainly gets executed would be the one file
/// nobody checked.
fn check_entry(
    field: &str,
    value: &str,
    files: &BTreeMap<String, String>,
) -> Result<String, String> {
    let normalised = safe_relative_segments(value)
        .ok_or_else(|| format!("{field} is not a path inside the plugin: {}", quote(value)))?;
    if !files.contains_key(&normalised) {
        return Err(format!(
            "{field} is {}, which files does not list, so it would run unverified",
            quote(&normalised)
        ));
    }
    Ok(normalised)
}

/// Echo an untrusted value into a message without letting it take the message
/// over: bounded length, and no control characters reaching a log or a readout.
fn quote(value: &str) -> String {
    const LIMIT: usize = 64;
    let cleaned: String = value
        .chars()
        .take(LIMIT)
        .map(|character| {
            if character.is_control() {
                '?'
            } else {
                character
            }
        })
        .collect();
    if value.chars().count() > LIMIT {
        format!("\"{cleaned}…\"")
    } else {
        format!("\"{cleaned}\"")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest with nothing wrong with it, as a starting point for the cases
    /// that break one thing at a time.
    fn good_manifest() -> serde_json::Value {
        serde_json::json!({
            "manifestVersion": 1,
            "id": "polis",
            "name": "Polis",
            "version": "0.1.0",
            "entry": { "ui": "ui/index.html", "backend": "polis-backend.exe" },
            "capabilities": ["oracle.search", "workspace.root"],
            "files": {
                "ui/index.html": "a".repeat(64),
                "polis-backend.exe": "b".repeat(64),
            }
        })
    }

    fn parse(value: &serde_json::Value) -> Result<PluginManifest, String> {
        parse_manifest(value.to_string().as_bytes(), "polis")
    }

    #[test]
    fn a_well_formed_manifest_survives_intact() {
        let manifest = parse(&good_manifest()).expect("this manifest is meant to be accepted");
        assert_eq!(manifest.id, "polis");
        assert_eq!(manifest.name, "Polis");
        assert_eq!(manifest.ui_entry, "ui/index.html");
        assert_eq!(manifest.backend_entry.as_deref(), Some("polis-backend.exe"));
        assert_eq!(
            manifest.capabilities,
            vec!["oracle.search", "workspace.root"]
        );
        assert_eq!(manifest.files.len(), 2);
    }

    #[test]
    fn a_plugin_without_a_backend_is_allowed() {
        let mut value = good_manifest();
        value["entry"] = serde_json::json!({ "ui": "ui/index.html" });
        value["files"] = serde_json::json!({ "ui/index.html": "a".repeat(64) });
        let manifest = parse(&value).expect("a UI-only plugin is a legitimate plugin");
        assert_eq!(manifest.backend_entry, None);
    }

    /// What is being broken, the edit that breaks it, and a fragment the
    /// refusal has to contain.
    type BrokenCase = (
        &'static str,
        Box<dyn Fn(&mut serde_json::Value)>,
        &'static str,
    );

    /// Each case names the field it breaks and a fragment the message must
    /// contain, so a refusal that starts happening for a different reason than
    /// intended fails here instead of passing quietly.
    #[test]
    fn every_rule_refuses_for_the_reason_it_exists() {
        let cases: Vec<BrokenCase> = vec![
            (
                "a version from the future",
                Box::new(|value| value["manifestVersion"] = serde_json::json!(2)),
                "understands 1",
            ),
            (
                "an id that is not a usable directory name",
                Box::new(|value| value["id"] = serde_json::json!("Polis/../etc")),
                "lowercase letters",
            ),
            (
                "an id that does not match where it is installed",
                Box::new(|value| value["id"] = serde_json::json!("oracle")),
                "installed in",
            ),
            (
                "a name that could forge a line of the interface",
                Box::new(|value| value["name"] = serde_json::json!("Polis\nverified")),
                "control character",
            ),
            (
                "an entry point nobody checksummed",
                Box::new(|value| value["entry"]["ui"] = serde_json::json!("ui/other.js")),
                "would run unverified",
            ),
            (
                "an entry point the browser would refuse to parse",
                Box::new(|value| {
                    value["entry"]["ui"] = serde_json::json!("ui/index.js");
                    value["files"] = serde_json::json!({
                        "ui/index.js": "a".repeat(64),
                        "polis-backend.exe": "b".repeat(64),
                    });
                }),
                ".html",
            ),
            (
                "a listed path that climbs out of the plugin",
                Box::new(|value| {
                    value["files"] = serde_json::json!({
                        "ui/index.html": "a".repeat(64),
                        "../../evil.dll": "b".repeat(64),
                    });
                    value["entry"] = serde_json::json!({ "ui": "ui/index.html" });
                }),
                "outside the plugin",
            ),
            (
                "two spellings of one file",
                Box::new(|value| {
                    value["files"] = serde_json::json!({
                        "ui/index.html": "a".repeat(64),
                        "ui//index.html": "b".repeat(64),
                    });
                    value["entry"] = serde_json::json!({ "ui": "ui/index.html" });
                }),
                "the same file",
            ),
            (
                "a manifest pretending to cover itself",
                Box::new(|value| value["files"]["plugin.json"] = serde_json::json!("c".repeat(64))),
                "own digest",
            ),
            (
                "a digest that is not one",
                Box::new(|value| value["files"]["ui/index.html"] = serde_json::json!("abc")),
                "64 hexadecimal",
            ),
            (
                "a field nobody recognises",
                Box::new(|value| value["autorun"] = serde_json::json!(true)),
                "is not valid",
            ),
            (
                "no files at all",
                Box::new(|value| {
                    value["files"] = serde_json::json!({});
                }),
                "nothing to verify",
            ),
        ];

        for (what, break_it, expected) in cases {
            let mut value = good_manifest();
            break_it(&mut value);
            match parse(&value) {
                Ok(_) => panic!("{what} was accepted"),
                Err(message) => assert!(
                    message.contains(expected),
                    "{what} was refused, but for the wrong reason: {message}"
                ),
            }
        }
    }

    #[test]
    fn an_uppercase_digest_is_folded_rather_than_refused() {
        let mut value = good_manifest();
        value["files"]["ui/index.html"] = serde_json::json!("A".repeat(64));
        let manifest = parse(&value).expect("Get-FileHash output must be usable");
        assert_eq!(manifest.files["ui/index.html"], "a".repeat(64));
    }

    #[test]
    fn a_repeated_capability_is_asked_for_once() {
        let mut value = good_manifest();
        value["capabilities"] = serde_json::json!(["oracle.search", "oracle.search"]);
        let manifest = parse(&value).expect("a duplicate is sloppy, not hostile");
        assert_eq!(manifest.capabilities, vec!["oracle.search"]);
    }

    #[test]
    fn a_hostile_value_cannot_take_over_the_message_it_appears_in() {
        let long = "z".repeat(500);
        let quoted = quote(&long);
        assert!(quoted.chars().count() < 80, "unbounded echo: {quoted}");
        assert_eq!(quote("a\nb"), "\"a?b\"");
    }
}
