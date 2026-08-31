//! Putting a plugin where the app will look for it.
//!
//! There is nothing to download yet — nobody publishes plugins — so the only
//! source that can exist today is a folder on this machine. That is a smaller
//! difference than it sounds: fetching an archive would add a download and an
//! unpack in front of this, and everything from "verify what is about to be
//! installed" onwards would be unchanged.
//!
//! ## Verify the copy, not the choice
//!
//! The obvious order — check the folder the user picked, then copy it — checks
//! bytes that are not the ones that end up installed. So the folder is copied
//! into a staging area first, the *staging area* is verified, and only then is
//! it moved into place. A plugin that fails verification never becomes a
//! directory under `plugins/`, so there is no half-installed state for the rest
//! of the app to reason about.
//!
//! The staging area is a **sibling** of the plugins directory, not a child.
//! Discovery treats every directory inside `plugins/` as a candidate, so a
//! half-copied one would be listed and refused while it was still being
//! written.
//!
//! ## Replacing something that already works
//!
//! An update moves the existing directory aside before moving the new one in,
//! and moves it back if that fails. Losing a working plugin to a failed update
//! is a worse outcome than the update not happening.

use std::path::{Path, PathBuf};

use super::discovery::{self, MAX_PLUGIN_BYTES};
use super::manifest::{check_id, MANIFEST_FILE_NAME, MAX_PLUGIN_FILES};

/// Where a plugin is assembled before it is allowed to exist.
fn staging_root(plugins_root: &Path) -> PathBuf {
    plugins_root.with_file_name("plugins-staging")
}

/// Copy `source` into the plugin directory named `id`, if it verifies.
///
/// The error is a sentence for a person, for the same reason the manifest's is:
/// every caller shows it and nothing branches on it.
pub fn install_from_directory(plugins_root: &Path, id: &str, source: &Path) -> Result<(), String> {
    // Before anything is joined to a path. An id is a directory name here, so
    // an unchecked one is a write wherever it points.
    check_id(id)?;

    let source = std::fs::canonicalize(source)
        .map_err(|error| format!("{} could not be read: {error}", source.display()))?;
    if !source.is_dir() {
        return Err(format!("{} is not a folder", source.display()));
    }
    // Installing a plugin from inside the plugin directory would have the swap
    // move a directory into itself, or delete the source as a leftover.
    if let Ok(root) = std::fs::canonicalize(plugins_root) {
        if source.starts_with(&root) {
            return Err("that folder is already inside Devboule's plugin directory".to_string());
        }
    }
    if !source.join(MANIFEST_FILE_NAME).is_file() {
        return Err(format!(
            "there is no {MANIFEST_FILE_NAME} in that folder, so it is not a plugin"
        ));
    }

    // Bounded before a single byte is copied: the folder is the user's choice
    // and nothing yet says it is small. `list_files` refuses links and
    // unaddressable names too, so a folder that could never verify is turned
    // away before it is duplicated.
    let files = discovery::list_files(&source)?;
    if files.len() > MAX_PLUGIN_FILES {
        return Err(format!(
            "that folder holds {} files, more than the {MAX_PLUGIN_FILES} a plugin may have",
            files.len()
        ));
    }
    let total: u64 = files.values().sum();
    if total > MAX_PLUGIN_BYTES {
        return Err(format!(
            "that folder holds {total} bytes, more than the {MAX_PLUGIN_BYTES} a plugin may have"
        ));
    }

    let staging = staging_root(plugins_root);
    let pending = staging.join(id);
    // A previous install that died halfway leaves this behind. It is ours, and
    // it is not the plugin directory.
    remove_tree(&pending)?;
    std::fs::create_dir_all(&pending)
        .map_err(|error| format!("could not prepare the install: {error}"))?;

    let copied = copy_into(&source, &pending, files.keys());
    let verified = copied.and_then(|()| discovery::verify(&pending, id));
    if let Err(reason) = verified {
        // Nothing was put in place, so there is nothing to undo but the copy.
        let _ = std::fs::remove_dir_all(&pending);
        return Err(reason);
    }

    swap_into_place(plugins_root, &staging, &pending, id)
}

/// Copy the manifest and every listed file, creating the directories on the way.
///
/// `plugin.json` is copied separately because [`discovery::list_files`] leaves
/// it out — it is the thing doing the describing.
fn copy_into<'a>(
    source: &Path,
    pending: &Path,
    relatives: impl Iterator<Item = &'a String>,
) -> Result<(), String> {
    let copy_one = |relative: &str| -> Result<(), String> {
        let to = pending.join(relative);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
        std::fs::copy(source.join(relative), &to)
            .map(|_| ())
            .map_err(|error| format!("could not copy {relative}: {error}"))
    };
    copy_one(MANIFEST_FILE_NAME)?;
    for relative in relatives {
        copy_one(relative)?;
    }
    Ok(())
}

/// Move the verified copy into the plugins directory, keeping whatever was
/// there until the move has succeeded.
fn swap_into_place(
    plugins_root: &Path,
    staging: &Path,
    pending: &Path,
    id: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(plugins_root)
        .map_err(|error| format!("could not create the plugin directory: {error}"))?;
    let target = plugins_root.join(id);
    let previous = staging.join(format!("{id}.previous"));
    remove_tree(&previous)?;

    // `symlink_metadata`, not `exists`: a link where the plugin should be is
    // something to move aside as it is, not something to follow.
    let had_previous = std::fs::symlink_metadata(&target).is_ok();
    if had_previous {
        std::fs::rename(&target, &previous).map_err(|error| {
            format!("could not set the installed {id} aside to replace it: {error}")
        })?;
    }
    if let Err(error) = std::fs::rename(pending, &target) {
        if had_previous {
            // The update failed; the version that worked goes back.
            let _ = std::fs::rename(&previous, &target);
        }
        return Err(format!("could not put {id} in place: {error}"));
    }
    let _ = std::fs::remove_dir_all(&previous);
    Ok(())
}

/// Remove a directory we own, treating "it was not there" as success.
fn remove_tree(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not clear {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const UI: &[u8] = b"export const surface = 1;\n";

    fn digest_of(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// A folder a user could have unpacked, correct unless told otherwise.
    fn unpacked(at: &Path, id: &str, ui: &[u8]) -> PathBuf {
        let directory = at.join(format!("{id}-download"));
        std::fs::create_dir_all(directory.join("ui")).expect("mkdir");
        std::fs::write(directory.join("ui/index.html"), ui).expect("write");
        let manifest = serde_json::json!({
            "manifestVersion": 1,
            "id": id,
            "name": "Polis",
            "version": "0.1.0",
            "entry": { "ui": "ui/index.html" },
            "files": { "ui/index.html": digest_of(ui) },
        });
        std::fs::write(
            directory.join(MANIFEST_FILE_NAME),
            serde_json::to_vec(&manifest).expect("json"),
        )
        .expect("write");
        directory
    }

    #[test]
    fn a_good_folder_becomes_an_installed_plugin() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("data/plugins");
        let source = unpacked(temp.path(), "polis", UI);

        install_from_directory(&root, "polis", &source).expect("this folder should install");

        let scan = discovery::scan(&root);
        assert!(scan.ready("polis").is_some(), "installed but not verified");
        assert_eq!(
            std::fs::read(root.join("polis/ui/index.html")).expect("read"),
            UI
        );
        assert!(
            !staging_root(&root).join("polis").exists(),
            "the staging copy was left behind"
        );
    }

    #[test]
    fn a_folder_that_would_not_verify_installs_nothing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("data/plugins");
        let source = unpacked(temp.path(), "polis", UI);
        // The manifest now describes a file that is not what is there.
        std::fs::write(source.join("ui/index.html"), b"tampered\n").expect("write");

        let refusal = install_from_directory(&root, "polis", &source)
            .expect_err("a folder whose digests disagree must not install");
        assert!(refusal.contains("ui/index.html"), "wrong reason: {refusal}");
        assert!(
            !root.join("polis").exists(),
            "a refused plugin must leave nothing behind"
        );
    }

    #[test]
    fn a_failed_update_leaves_the_working_version_installed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("data/plugins");
        let good = unpacked(temp.path(), "polis", UI);
        install_from_directory(&root, "polis", &good).expect("first install");

        // A second folder that will fail verification, offered as an update.
        let broken = unpacked(
            &temp.path().join("second"),
            "polis",
            b"export const two = 2;\n",
        );
        std::fs::write(broken.join("ui/index.html"), b"tampered\n").expect("write");
        install_from_directory(&root, "polis", &broken).expect_err("the update must be refused");

        let scan = discovery::scan(&root);
        assert!(
            scan.ready("polis").is_some(),
            "a refused update took the working plugin with it"
        );
        assert_eq!(
            std::fs::read(root.join("polis/ui/index.html")).expect("read"),
            UI,
            "the installed files are not the ones that were working"
        );
    }

    #[test]
    fn an_update_replaces_every_file_rather_than_merging() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("data/plugins");
        install_from_directory(&root, "polis", &unpacked(temp.path(), "polis", UI))
            .expect("first install");
        // Something the first version shipped and the second does not. Merged
        // in, it would be an unlisted file and the plugin would stop verifying.
        std::fs::write(root.join("polis/ui/old.js"), b"export const old = 1;\n").expect("write");

        let second = unpacked(
            &temp.path().join("second"),
            "polis",
            b"export const two = 2;\n",
        );
        install_from_directory(&root, "polis", &second).expect("second install");

        assert!(
            !root.join("polis/ui/old.js").exists(),
            "the previous version's files survived the update"
        );
        assert!(discovery::scan(&root).ready("polis").is_some());
    }

    #[test]
    fn an_id_that_is_not_a_directory_name_is_refused_before_any_path_is_built() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("data/plugins");
        let source = unpacked(temp.path(), "polis", UI);
        for hostile in ["../escape", "..", "C:", "Polis", ""] {
            let refusal = install_from_directory(&root, hostile, &source)
                .expect_err("a name that cannot be a directory must not install");
            assert!(
                refusal.contains("id must") && refusal.contains(&format!("\"{hostile}\"")),
                "{hostile:?} was refused for the wrong reason: {refusal}"
            );
        }
        assert!(!temp.path().join("escape").exists());
    }

    #[test]
    fn a_folder_without_a_manifest_says_so() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("data/plugins");
        let source = temp.path().join("just-files");
        std::fs::create_dir_all(&source).expect("mkdir");
        std::fs::write(source.join("index.js"), UI).expect("write");

        let refusal =
            install_from_directory(&root, "polis", &source).expect_err("not a plugin folder");
        assert!(
            refusal.contains(MANIFEST_FILE_NAME),
            "wrong reason: {refusal}"
        );
    }

    #[test]
    fn a_folder_already_inside_the_plugin_directory_is_refused() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("data/plugins");
        install_from_directory(&root, "polis", &unpacked(temp.path(), "polis", UI))
            .expect("first install");

        let refusal = install_from_directory(&root, "polis", &root.join("polis"))
            .expect_err("installing a plugin over itself must be refused");
        assert!(
            refusal.contains("already inside"),
            "wrong reason: {refusal}"
        );
        assert!(discovery::scan(&root).ready("polis").is_some());
    }
}
