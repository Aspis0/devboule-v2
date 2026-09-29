//! Daemon-side pins for the vendored portable-pty hardening and wiring.
//!
//! The vendored crate is not a workspace member, so its in-crate test never
//! runs under `cargo test --workspace`. These tests assert the same behaviour
//! through the patched crate our spawns actually link
//! (`vendor/portable-pty.patches.md` tracks the patches); pin the patch
//! listing and the `[patch.crates-io]` resolution; and pin content integrity
//! for every vendored file the patches touch (today that is only
//! `src/cmdbuilder.rs`: sequenced reverse-apply of the stack plus sha256
//! against a pinned pristine hash — every other vendored file is pinned by
//! existence only). The integrity test shells out to `git`, which must be
//! on `PATH`.

#[cfg(windows)]
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("daemon manifest lives two levels below the workspace root")
        .to_path_buf()
}

#[cfg(windows)]
#[test]
fn pty_environment_block_omits_entries_without_a_valid_record() {
    use std::os::windows::ffi::OsStrExt;
    let mut cmd = portable_pty::CommandBuilder::new("dummy");
    cmd.env_clear();
    cmd.env("", "ignored");
    cmd.env(OsString::from("B\0AD"), "ignored");
    cmd.env("=", "ignored");
    cmd.env("MULTI", OsString::from("a\0b"));
    cmd.env("BAD=KEY", "ignored");
    cmd.env("=::", "colon");
    cmd.env("=C:", r"C:\valid");
    cmd.env("=ExitCode", "hidden");
    cmd.env("EMPTY", "");
    cmd.env("VALID", "ok");

    let expected: Vec<u16> =
        OsStr::new("=::=colon\0=C:=C:\\valid\0=ExitCode=hidden\0EMPTY=\0VALID=ok\0\0")
            .encode_wide()
            .collect();
    assert_eq!(cmd.environment_block(), expected);

    cmd.env_remove("=::");
    cmd.env_remove("=C:");
    cmd.env_remove("=ExitCode");
    cmd.env_remove("EMPTY");
    cmd.env_remove("VALID");
    assert_eq!(cmd.environment_block(), vec![0, 0]);
}

#[cfg(windows)]
#[test]
fn registry_string_values_reject_malformed_types() {
    use portable_pty::cmdbuilder::reg_value_to_string;
    use winreg::enums::RegType;
    use winreg::RegValue;

    let multi_string = RegValue {
        bytes: vec![b'a', 0, 0, 0],
        vtype: RegType::REG_MULTI_SZ,
    };
    assert!(reg_value_to_string(&multi_string).is_err());
    let malformed_expand_string = RegValue {
        bytes: vec![b'%', 0, b'P'],
        vtype: RegType::REG_EXPAND_SZ,
    };
    assert!(reg_value_to_string(&malformed_expand_string).is_err());
    let expandable_string = RegValue {
        bytes: "literal\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect(),
        vtype: RegType::REG_EXPAND_SZ,
    };
    assert_eq!(
        reg_value_to_string(&expandable_string).unwrap(),
        OsString::from("literal")
    );
}

#[test]
fn vendored_portable_pty_tree_matches_its_declared_patches() {
    let root = workspace_root();
    let vendored = root.join("vendor").join("portable-pty");
    for relative in [
        "Cargo.toml",
        "LICENSE.md",
        "src/lib.rs",
        "src/cmdbuilder.rs",
        "src/win/psuedocon.rs",
    ] {
        assert!(
            vendored.join(relative).is_file(),
            "vendored tree is missing {relative}"
        );
    }

    let workspace_toml =
        std::fs::read_to_string(root.join("Cargo.toml")).expect("root Cargo.toml is readable");
    assert!(
        workspace_toml.contains("[patch.crates-io]"),
        "root Cargo.toml lost its [patch.crates-io] section"
    );
    assert!(
        workspace_toml.contains("portable-pty = { path = \"vendor/portable-pty\" }"),
        "root Cargo.toml no longer patches portable-pty at vendor/portable-pty"
    );

    let index = root.join("vendor").join("portable-pty.patches.md");
    let index_text =
        std::fs::read_to_string(&index).expect("vendor/portable-pty.patches.md is readable");
    let patch_dir = root.join("vendor").join("patches").join("portable-pty");
    let mut on_disk = vec![];
    for entry in std::fs::read_dir(&patch_dir).expect("patch dir is readable") {
        let path = entry.expect("patch dir entry is readable").path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("patch") {
            on_disk.push(path);
        }
    }
    assert!(
        !on_disk.is_empty(),
        "no patches on disk; the vendored tree would be pristine upstream"
    );
    for path in &on_disk {
        let relative = path
            .strip_prefix(&root)
            .expect("patch is under the workspace root");
        let relative = relative.to_string_lossy().replace('\\', "/");
        let wanted = format!("patch: `{relative}`");
        assert!(
            index_text.lines().any(|line| line == wanted),
            "{relative} is on disk but not listed in portable-pty.patches.md"
        );
    }
    let mut listed = 0;
    for line in index_text.lines() {
        if let Some(name) = line
            .strip_prefix("patch: `vendor/patches/portable-pty/")
            .and_then(|rest| rest.strip_suffix('`'))
        {
            listed += 1;
            assert!(
                patch_dir.join(name).is_file(),
                "portable-pty.patches.md lists {name}, which is not on disk"
            );
        }
    }
    assert!(
        listed >= on_disk.len() && listed > 0,
        "portable-pty.patches.md lists {listed} patches for {} on disk",
        on_disk.len()
    );

    let cmdbuilder = std::fs::read_to_string(vendored.join("src").join("cmdbuilder.rs"))
        .expect("vendored cmdbuilder.rs is readable");
    for marker in [
        "embedded null in registry environment variable",
        "Vendored-patch region (0003 filter, 0004 `=` hardening)",
    ] {
        assert!(
            cmdbuilder.contains(marker),
            "vendored cmdbuilder.rs lost its declared patch marker: {marker}"
        );
    }
    assert!(
        !cmdbuilder.contains("raw_arg"),
        "vendored cmdbuilder.rs gained the undeclared raw-arg feature"
    );
}

#[test]
fn vendored_portable_pty_resolves_to_the_vendored_tree() {
    let root = workspace_root();
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--locked", "--offline", "--format-version", "1"])
        .current_dir(&root)
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout
            .matches("\"name\":\"portable-pty\",\"version\"")
            .count(),
        1,
        "expected exactly one portable-pty package in the resolve"
    );
    let expected = root
        .join("vendor")
        .join("portable-pty")
        .join("Cargo.toml")
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    assert!(
        stdout
            .to_lowercase()
            .replace("\\\\", "/")
            .contains(&expected),
        "portable-pty did not resolve to the vendored tree"
    );
}

/// Pristine crates.io bytes, sha256, per vendored path the declared patches
/// touch. The stack-integrity test reverse-applies the patches and must land
/// on exactly these bytes; a future patch that touches a new file must add
/// its pin here.
const PRISTINE_HASHES: &[(&str, &str)] = &[(
    "vendor/portable-pty/src/cmdbuilder.rs",
    "db95387276f7f5a2e6bfd27a0c21ad99090f5d29e3663ad0446becec86748283",
)];

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("scratch dirs are writable");
    for entry in std::fs::read_dir(src).expect("vendored tree is readable") {
        let entry = entry.expect("vendored entry is readable");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("file type is readable").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("vendored file is copyable");
        }
    }
}

#[test]
fn vendored_cmdbuilder_is_pristine_plus_declared_patches() {
    let root = workspace_root();
    let scratch = crate::test_dirs::test_temp_dir("devboule-pty-stack");
    copy_tree(
        &root.join("vendor").join("portable-pty"),
        &scratch.join("vendor").join("portable-pty"),
    );
    let patch_dir = root.join("vendor").join("patches").join("portable-pty");
    let mut patches: Vec<_> = std::fs::read_dir(&patch_dir)
        .expect("patch dir is readable")
        .filter_map(|entry| {
            let path = entry.expect("patch dir entry is readable").path();
            (path.extension().and_then(|ext| ext.to_str()) == Some("patch")).then_some(path)
        })
        .collect();
    patches.sort();
    patches.reverse();
    assert!(!patches.is_empty(), "no patches to reverse");
    let mut touched: Vec<String> = vec![];
    for patch in &patches {
        let text = std::fs::read_to_string(patch).expect("patch file is readable");
        for line in text.lines() {
            if let Some(path) = line.strip_prefix("+++ b/") {
                if !touched.iter().any(|seen| seen == path) {
                    touched.push(path.to_string());
                }
            }
        }
        // Outside the repository there is no .gitattributes, so pin off the
        // platform default (core.autocrlf=true writes CRLF here): the
        // committed blobs are LF and the hash below is over LF bytes.
        let reverse = std::process::Command::new("git")
            .args([
                "-c",
                "core.autocrlf=false",
                "apply",
                "-R",
                "-p1",
                patch.to_str().expect("patch path is UTF-8"),
            ])
            .current_dir(&scratch)
            .output()
            .expect("git apply runs");
        assert!(
            reverse.status.success(),
            "{} does not reverse-apply: {}",
            patch.display(),
            String::from_utf8_lossy(&reverse.stderr)
        );
    }
    assert!(
        !touched.is_empty(),
        "declared patches touch no file; nothing to compare"
    );
    for path in &touched {
        let (_, expected) = PRISTINE_HASHES
            .iter()
            .find(|(pinned, _)| pinned == path)
            .expect("touched file has no pristine pin; add it with the patch");
        let reversed = std::fs::read(scratch.join(path)).expect("reversed file is readable");
        assert_eq!(
            crate::attachment_store::sha256_hex(&reversed),
            *expected,
            "{path} is not pristine 0.9.0 plus the declared patches"
        );
    }
    let _ = std::fs::remove_dir_all(&scratch);
}
