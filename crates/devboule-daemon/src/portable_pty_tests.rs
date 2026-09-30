//! Daemon-side pins for the vendored portable-pty hardening and wiring.
//!
//! The vendored crate is not a workspace member, so its in-crate test never
//! runs under `cargo test --workspace`. These tests assert the loader's
//! user-facing behaviour through the patched crate our spawns actually link —
//! the production `openpty` and `conpty_source` — so the gate runs them
//! (`vendor/portable-pty.patches.md` tracks the patches); the vendored crate's
//! own tests keep only what the public API cannot reach: `choose_conpty` and
//! the bundle-gate predicate with synthetic directories, which a daemon test
//! cannot drive because the selection is a process-wide one-shot. These tests
//! also pin the patch listing and the `[patch.crates-io]` resolution; and pin
//! content integrity for every vendored file a patch modifies — every entry of
//! `PRISTINE_HASHES`, sequenced reverse-apply of the stack plus sha256 against
//! the pinned pristine bytes — and pin to absence the files a patch creates,
//! named in `PRISTINE_ABSENT`. The integrity test shells out to `git`, which
//! must be on `PATH`.

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

// The exact phrases the daemon's startup line carries; docs/conpty-windows.md
// quotes them.
#[cfg(windows)]
const INBOX_SOURCE: &str = "the Windows inbox ConPTY (kernel32)";
#[cfg(windows)]
const APP_LOCAL_SOURCE: &str = "the app-local ConPTY bundle beside the executable";

// Drains the master and answers the startup cursor-position query while the
// child runs: a `cmd /c` child stalls forever on an unanswered `ESC[6n`. The
// raw `ESC[1;1R` reply is the ACP terminal reader's shape (acp_host.rs, with
// the same three-byte carry so a query split across reads is still seen);
// regular sessions get their reply from the emulator instead, routed back by
// the daemon (provider.rs) — a client must never answer there.
#[cfg(windows)]
fn drain_and_answer_dsr(
    reader: Box<dyn std::io::Read + Send>,
    pair: &portable_pty::PtyPair,
) -> std::thread::JoinHandle<()> {
    let writer = std::sync::Arc::new(std::sync::Mutex::new(
        pair.master.take_writer().expect("writer is available"),
    ));
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        let mut reader = reader;
        let mut buf = [0u8; 8192];
        let mut tail: Vec<u8> = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    tail.extend_from_slice(&buf[..count]);
                    if tail.windows(4).any(|window| window == b"\x1b[6n") {
                        if let Ok(mut writer) = writer.lock() {
                            let _ = writer.write_all(b"\x1b[1;1R");
                            let _ = writer.flush();
                        }
                    }
                    if tail.len() > 3 {
                        tail.drain(..tail.len() - 3);
                    }
                }
            }
        }
    })
}

// Spawns `cmd /c exit 0` through the pair and asserts it exits with code 0
// within 30 s, closing the pseudoconsole before joining the drainer (the
// inbox host holds the output pipe until ClosePseudoConsole).
#[cfg(windows)]
fn child_exits_zero_through(pair: portable_pty::PtyPair) {
    let mut cmd = portable_pty::CommandBuilder::new("cmd.exe");
    cmd.arg("/c");
    cmd.arg("exit 0");
    let cwd = crate::test_dirs::test_temp_dir("devboule-pty-child");
    cmd.cwd(&cwd);
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .expect("child spawns through ConPTY");
    let reader = pair.master.try_clone_reader().expect("reader clones");
    let drainer = drain_and_answer_dsr(reader, &pair);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = loop {
        match child.try_wait().expect("try_wait works") {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                panic!("the child did not exit within 30s");
            }
            None => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    };
    assert_eq!(status.exit_code(), 0);
    drop(pair);
    drainer.join().expect("the drainer thread ends");
    let _ = std::fs::remove_dir_all(&cwd);
}

// The gate-environment counterpart of the ignored app-local test below: this
// executable's directory stages no bundle, so the production choice is the
// inbox one, and the assertion fails loudly if that ever stops holding.
#[cfg(windows)]
#[test]
fn a_real_pseudoconsole_through_the_production_openpty_exits_on_the_inbox_conpty() {
    assert_eq!(portable_pty::conpty_source(), INBOX_SOURCE);
    let pair = portable_pty::native_pty_system()
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty opens through the production seam");
    child_exits_zero_through(pair);
}

// The positive half the rejection tests cannot cover: with a staged bundle
// beside this executable, the production choice is the app-local one and a
// real child still exits through it. It skips only when nothing at all is
// staged, so an --ignored run over an unstaged tree stays green; a partial or
// mislaid bundle fails instead, naming the missing path. The scratch
// CARGO_TARGET_DIR in the recipe matters: the selection is keyed on this
// executable's directory, and staging into the gate's own target would turn
// the gated inbox test above red.
#[cfg(windows)]
#[test]
#[ignore = "needs a staged ConPTY bundle beside this test executable; in one PowerShell session at the repo root: $env:CARGO_TARGET_DIR = 'C:\\tmp\\u11-pty-target'; cargo test -p devboule-daemon --lib --no-run; powershell -NoProfile -ExecutionPolicy Bypass -File scripts\\stage-conpty.ps1 -TargetDir C:\\tmp\\u11-pty-target\\debug\\deps; cargo test -p devboule-daemon --lib portable_pty_tests::a_real_pseudoconsole_exits_through_a_staged_app_local_bundle -- --ignored --exact"]
fn a_real_pseudoconsole_exits_through_a_staged_app_local_bundle() {
    let exe_dir = std::env::current_exe()
        .expect("test executable path is known")
        .parent()
        .expect("test executable has a parent directory")
        .to_path_buf();
    let dll = exe_dir.join("conpty").join("conpty.dll");
    let host = exe_dir.join("conpty").join("x64").join("OpenConsole.exe");
    match (dll.is_file(), host.is_file()) {
        (false, false) => {
            eprintln!(
                "skipping: nothing is staged beside {} (expected {} and {})",
                exe_dir.display(),
                dll.display(),
                host.display()
            );
            return;
        }
        (true, false) => panic!(
            "a staged bundle is incomplete: {} is missing",
            host.display()
        ),
        (false, true) => panic!(
            "a staged bundle is incomplete: {} is missing",
            dll.display()
        ),
        (true, true) => {}
    }
    let source = portable_pty::conpty_source();
    assert_eq!(
        source,
        APP_LOCAL_SOURCE,
        "the staged bundle beside {} was not chosen; the loader reported: {source}",
        exe_dir.display()
    );
    let pair = portable_pty::native_pty_system()
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty opens through the staged bundle");
    child_exits_zero_through(pair);
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
        "src/win/conpty_loader.rs",
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

/// Pristine crates.io bytes, sha256, per vendored path a declared patch
/// modifies. The stack-integrity test reverse-applies the patches and must
/// land on exactly these bytes.
const PRISTINE_HASHES: &[(&str, &str)] = &[
    (
        "vendor/portable-pty/src/cmdbuilder.rs",
        "db95387276f7f5a2e6bfd27a0c21ad99090f5d29e3663ad0446becec86748283",
    ),
    (
        "vendor/portable-pty/Cargo.toml",
        "e1a7320efbb4b088352f5ac87c805e2ad96df571b994ac1397e6d6ffdf981c61",
    ),
    (
        "vendor/portable-pty/src/win/mod.rs",
        "c983e63d15800f73ac05105a69ec2b53fea482c066ea5559682bd024561f999a",
    ),
    (
        "vendor/portable-pty/src/win/psuedocon.rs",
        "297d0622d7b0401708f1ce3b92076632dd5a0c188b4a8d09b2f4891190b0e392",
    ),
    (
        "vendor/portable-pty/src/lib.rs",
        "8b1d45c520f17c8be2d51069bb6ed46a6fa612d0395bc55a5b6e0ef30cc1c004",
    ),
];

/// Vendored paths a declared patch *creates*. Reversing the stack deletes
/// them again, so absence is their pinned pristine state: each listed path
/// must NOT survive the reverse, every other touched path must and is
/// hash-pinned in `PRISTINE_HASHES`.
const PRISTINE_ABSENT: &[&str] = &["vendor/portable-pty/src/win/conpty_loader.rs"];

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
    let mut removed: Vec<String> = vec![];
    for patch in &patches {
        let text = std::fs::read_to_string(patch).expect("patch file is readable");
        // git apply skips everything before the first `---`, so a malformed
        // header — two subjects, no separator — would pass the reverse-apply
        // below while the patch says something else.
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with("Subject: [PATCH]"))
                .count(),
            1,
            "{} must carry exactly one Subject: [PATCH] line",
            patch.display()
        );
        let separator = text.lines().position(|line| line == "---");
        let first_diff = text.lines().position(|line| line.starts_with("diff --git"));
        assert!(
            matches!((separator, first_diff), (Some(separator), Some(diff)) if separator < diff),
            "{} has no header (a --- line before the first diff)",
            patch.display()
        );
        for line in text.lines() {
            if let Some(path) = line.strip_prefix("+++ b/") {
                if !touched.iter().any(|seen| seen == path) {
                    touched.push(path.to_string());
                }
            }
            // A deletion or rename emits `--- a/old` with no `+++ b/old`, so
            // the old path would otherwise drop out of the integrity net
            // entirely; hold it against `touched` below.
            if let Some(path) = line.strip_prefix("--- a/") {
                if !removed.iter().any(|seen| seen == path) {
                    removed.push(path.to_string());
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
    for path in &removed {
        assert!(
            touched.contains(path),
            "{path} is deleted or renamed by a patch but never re-created; it leaves \
             the integrity net — the patch stack must keep every vendored file present"
        );
    }
    for path in &touched {
        let reversed_path = scratch.join(path);
        if PRISTINE_ABSENT.contains(&path.as_str()) {
            assert!(
                !reversed_path.exists(),
                "{path} is declared patch-created but survived the reverse-apply"
            );
            continue;
        }
        let reversed = std::fs::read(&reversed_path).expect("reversed file is readable");
        let (_, expected) = PRISTINE_HASHES
            .iter()
            .find(|(pinned, _)| pinned == path)
            .expect("modified file has no pristine pin; add it with the patch");
        assert_eq!(
            crate::attachment_store::sha256_hex(&reversed),
            *expected,
            "{path} is not pristine 0.9.0 plus the declared patches"
        );
    }
    let _ = std::fs::remove_dir_all(&scratch);
}
