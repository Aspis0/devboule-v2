//! The plugin platform: what is installed, whether it may run, and how its
//! files reach the WebView.
//!
//! Polis is installed by the user rather than compiled in, so Devboule has to
//! answer three questions about code it did not build: what is there
//! ([`discovery`]), what does it claim about itself ([`manifest`]), and how do
//! its files get into the window ([`assets`]).
//!
//! The three are connected on purpose. Discovery produces one [`Scan`], and both
//! the readout the user sees and the set of files the asset server will serve
//! are derived from it. A plugin the user is told was refused is a plugin whose
//! files return 404, without a second decision anywhere that could disagree.

pub mod assets;
pub mod discovery;
pub mod install;
pub mod manifest;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use devboule_protocol::ErrorCode;
use tauri::{AppHandle, Manager, Runtime};

use crate::backend::error::CommandError;
use discovery::{scan, PluginInventory, Scan};

/// Where installed plugins live, or nothing if this machine will not say where
/// application data belongs.
pub fn plugins_root<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("plugins"))
}

/// The verified answer to "what is installed", computed once and reused.
///
/// Verification hashes every installed file, so it is not something to repeat
/// per request. The first caller pays for it — today that is whoever opens the
/// Polis surface, and if a plugin is installed it is also the first request for
/// one of its files. If that ever shows up as a pause, the fix is to warm this
/// in `setup` on a background thread; it is not worth the extra moving part
/// before there is a plugin to measure.
#[derive(Default)]
pub struct PluginRegistry {
    // A cache, and nothing else lives behind this lock, so a panic elsewhere
    // must not turn every later request into a panic of its own.
    cached: Mutex<Option<Scan>>,
    // Installs move directories around. Two of them at once, for the same id,
    // would race on that move, and the interface disabling a button is not a
    // guarantee — a second window or a repeated command is not the interface.
    // Deliberately not the same lock as the cache: an install must not stop the
    // asset server from answering for the plugins already installed.
    installs: Mutex<()>,
}

impl PluginRegistry {
    fn with_scan<T>(&self, root: &Path, use_it: impl FnOnce(&Scan) -> T) -> T {
        let mut cached = self
            .cached
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if cached.is_none() {
            *cached = Some(scan(root));
        }
        use_it(cached.as_ref().expect("just filled"))
    }

    /// What is installed, scanning if this is the first question asked.
    pub fn inventory(&self, root: &Path) -> PluginInventory {
        self.with_scan(root, Scan::inventory)
    }

    /// Look again. Installing a plugin does not restart the app, and being told
    /// to check is better than polling the disk.
    pub fn rescan(&self, root: &Path) -> PluginInventory {
        let fresh = scan(root);
        let inventory = fresh.inventory();
        *self
            .cached
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(fresh);
        inventory
    }

    /// May this exact file, of this exact plugin, be served?
    ///
    /// Both halves matter. The plugin must have passed verification, and the
    /// path must be one its manifest listed — otherwise a file dropped into the
    /// directory after the scan would be servable, and the manifest would be
    /// describing something other than what the window can load.
    pub fn is_verified_asset(&self, root: &Path, id: &str, relative: &str) -> bool {
        self.with_scan(root, |scan| {
            scan.ready(id)
                .is_some_and(|manifest| manifest.files.contains_key(relative))
        })
    }
}

/// Nothing is installed and nothing can be, because there is nowhere to look.
fn nowhere_to_look() -> PluginInventory {
    PluginInventory {
        root: String::new(),
        plugins: Vec::new(),
        problem: Some(
            "this machine did not say where application data belongs, so Devboule cannot tell \
             whether any plugin is installed"
                .to_string(),
        ),
    }
}

/// The inventory always has an answer, including "I could not look", so it is
/// not a `Result`: the surface renders one shape and reads `problem`, the same
/// way the graphics probe reports a machine that cannot draw.
#[tauri::command]
pub async fn plugins_list(app: AppHandle) -> PluginInventory {
    match plugins_root(&app) {
        Some(root) => app.state::<PluginRegistry>().inventory(&root),
        None => nowhere_to_look(),
    }
}

#[tauri::command]
pub async fn plugins_rescan(app: AppHandle) -> PluginInventory {
    match plugins_root(&app) {
        Some(root) => app.state::<PluginRegistry>().rescan(&root),
        None => nowhere_to_look(),
    }
}

/// Install a plugin from a folder, and answer with what is installed afterwards.
///
/// Unlike the two above this one *is* a `Result`: an install either happened or
/// it did not, and the caller has an action to report on rather than a readout
/// to draw. Every refusal shares one code — nothing branches on it, the sentence
/// is the payload, and a taxonomy no caller reads would be decoration.
#[tauri::command]
pub async fn plugin_install(
    app: AppHandle,
    id: String,
    source: String,
) -> Result<PluginInventory, CommandError> {
    let Some(root) = plugins_root(&app) else {
        return Err(CommandError::new(
            ErrorCode::Internal,
            "this machine did not say where application data belongs, so there is nowhere to \
             install a plugin",
        ));
    };
    let registry = app.state::<PluginRegistry>();
    {
        let _one_at_a_time = registry
            .installs
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        install::install_from_directory(&root, &id, std::path::Path::new(&source))
            .map_err(|reason| CommandError::new(ErrorCode::InvalidRequest, reason))?;
    }
    // The cache is now a description of a directory that changed underneath it.
    Ok(registry.rescan(&root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn install_one(root: &Path) {
        let directory = root.join("polis");
        std::fs::create_dir_all(directory.join("ui")).expect("mkdir");
        let ui = b"export const surface = 1;\n";
        std::fs::write(directory.join("ui/index.html"), ui).expect("write");
        let digest: String = Sha256::digest(ui)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let manifest = serde_json::json!({
            "manifestVersion": 1,
            "id": "polis",
            "name": "Polis",
            "version": "0.1.0",
            "entry": { "ui": "ui/index.html" },
            "files": { "ui/index.html": digest },
        });
        std::fs::write(
            directory.join("plugin.json"),
            serde_json::to_vec(&manifest).expect("json"),
        )
        .expect("write");
    }

    #[test]
    fn only_a_verified_file_of_a_verified_plugin_is_servable() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("plugins");
        install_one(&root);
        let registry = PluginRegistry::default();

        assert!(registry.is_verified_asset(&root, "polis", "ui/index.html"));
        // Listed by nobody.
        assert!(!registry.is_verified_asset(&root, "polis", "ui/other.js"));
        // A plugin that is not installed at all.
        assert!(!registry.is_verified_asset(&root, "pubvia", "ui/index.html"));
    }

    #[test]
    fn a_file_added_after_the_scan_is_not_servable_until_someone_looks_again() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("plugins");
        install_one(&root);
        let registry = PluginRegistry::default();
        assert!(registry.is_verified_asset(&root, "polis", "ui/index.html"));

        std::fs::write(root.join("polis/ui/extra.js"), b"export const x = 1;\n").expect("write");
        assert!(
            !registry.is_verified_asset(&root, "polis", "ui/extra.js"),
            "the cache is the point: a file appearing later is not vouched for"
        );

        // And once someone does look, the plugin is refused outright, because
        // the manifest no longer describes the directory.
        let inventory = registry.rescan(&root);
        assert!(!inventory.plugins[0].ready);
        assert!(!registry.is_verified_asset(&root, "polis", "ui/index.html"));
    }

    #[test]
    fn the_inventory_is_computed_once_and_then_reused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("plugins");
        install_one(&root);
        let registry = PluginRegistry::default();
        let first = registry.inventory(&root);

        // Break the plugin without telling the registry.
        std::fs::write(root.join("polis/ui/index.html"), b"tampered\n").expect("write");
        assert_eq!(
            registry.inventory(&root),
            first,
            "a second question must not re-hash the disk"
        );
        assert_ne!(
            registry.rescan(&root),
            first,
            "and asking explicitly must actually look"
        );
    }
}
