//! Detection over an injected probe: the order the caret menu lists, the
//! PATH entries this discovery may trust, and how a Windows command script
//! is resolved to its product or dropped.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::backend::editor_targets::{
    detect, resolve, EditorTargetKind, Probe, Resolved, PATH_SUFFIXES,
};

/// The Windows suffix set, injected as data so the command-script rules are
/// tested by their own inputs rather than by this host's environment.
const WINDOWS_PROBE: &[&str] = &[".exe", ".com", ".cmd", ".bat"];

fn make_file(path: &Path) -> PathBuf {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent directory");
    }
    std::fs::write(path, b"").expect("file");
    path.to_path_buf()
}

fn probe(
    path: Vec<PathBuf>,
    known: Vec<(&'static str, Vec<PathBuf>)>,
    bundles: Vec<PathBuf>,
    suffixes: &'static [&'static str],
    file_manager: Option<&'static str>,
) -> Probe {
    Probe {
        path,
        known,
        bundles,
        suffixes,
        file_manager,
    }
}

#[test]
fn detected_targets_follow_the_registry_order_with_the_file_manager_last() {
    let dir = TempDir::new().expect("temp");
    let base = dir.path();
    let known = vec![
        ("cursor", vec![make_file(&base.join("cursor/Cursor.exe"))]),
        ("vscode", vec![make_file(&base.join("vscode/Code.exe"))]),
        (
            "vscode-insiders",
            vec![make_file(&base.join("insiders/Code - Insiders.exe"))],
        ),
        (
            "vscodium",
            vec![make_file(&base.join("codium/VSCodium.exe"))],
        ),
        ("zed", vec![make_file(&base.join("zed/zed.exe"))]),
    ];
    let mut path_dirs = Vec::new();
    for stem in [
        "idea64",
        "webstorm64",
        "pycharm64",
        "rustrover64",
        "goland64",
        "clion64",
        "rider64",
        "phpstorm64",
        "rubymine64",
        "datagrip64",
    ] {
        let ide_dir = base.join(stem);
        make_file(&ide_dir.join(format!("{stem}{}", PATH_SUFFIXES[0])));
        path_dirs.push(ide_dir);
    }
    let found = detect(&probe(
        path_dirs,
        known,
        Vec::new(),
        PATH_SUFFIXES,
        Some("Explorer"),
    ));

    let ids: Vec<&str> = found.iter().map(|target| target.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "cursor",
            "vscode",
            "vscode-insiders",
            "vscodium",
            "zed",
            "idea",
            "webstorm",
            "pycharm",
            "rustrover",
            "goland",
            "clion",
            "rider",
            "phpstorm",
            "rubymine",
            "datagrip",
            "file-manager",
        ]
    );
    assert!(
        found[..15]
            .iter()
            .all(|target| target.kind == EditorTargetKind::Editor),
        "the editors are editors: {found:?}"
    );
    let last = found.last().expect("the file manager is a target here");
    assert_eq!(last.kind, EditorTargetKind::FileManager);
    assert_eq!(last.label, "Explorer");
}

#[test]
fn a_macos_application_bundle_is_detected_from_the_bundle_directories() {
    let dir = TempDir::new().expect("temp");
    std::fs::create_dir_all(dir.path().join("Zed.app")).expect("bundle");

    let found = detect(&probe(
        Vec::new(),
        Vec::new(),
        vec![dir.path().to_path_buf()],
        PATH_SUFFIXES,
        None,
    ));

    assert_eq!(
        found
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>(),
        ["zed"],
        "the bundle alone installs Zed"
    );
    match resolve(
        "zed",
        &probe(
            Vec::new(),
            Vec::new(),
            vec![dir.path().to_path_buf()],
            PATH_SUFFIXES,
            None,
        ),
    )
    .expect("the bundle resolves")
    {
        Resolved::Bundle { name } => assert_eq!(name, "Zed"),
        other => panic!("expected the bundle route, got {other:?}"),
    }
}

#[test]
fn a_command_script_resolves_to_the_real_executable_and_is_never_the_launch() {
    let dir = TempDir::new().expect("temp");
    let base = dir.path();
    let executable = make_file(&base.join("Microsoft VS Code/Code.exe"));
    let bin = base.join("Microsoft VS Code/bin");
    make_file(&bin.join("code.cmd"));

    let resolved = resolve(
        "vscode",
        &probe(
            vec![bin],
            vec![("vscode", vec![])],
            Vec::new(),
            WINDOWS_PROBE,
            None,
        ),
    )
    .expect("the script's product is installed");

    match &resolved {
        Resolved::Executable { program, .. } => {
            assert_eq!(
                program,
                &std::fs::canonicalize(&executable).expect("canonical product"),
                "the launcher is the executable itself, canonicalized — never the script"
            );
        }
        other => panic!("expected an executable, got {other:?}"),
    }
}

#[test]
fn a_command_script_with_no_real_executable_is_left_out() {
    let dir = TempDir::new().expect("temp");
    let bin = dir.path().join("lonely");
    make_file(&bin.join("code.cmd"));

    let found = resolve(
        "vscode",
        &probe(
            vec![bin],
            vec![("vscode", vec![])],
            Vec::new(),
            WINDOWS_PROBE,
            None,
        ),
    );

    assert!(
        found.is_none(),
        "a script with nothing real beside it is not a target: {found:?}"
    );
}

#[test]
fn a_relative_or_empty_path_entry_is_not_detected() {
    // Planted exactly where the two entries would resolve: a relative
    // entry — the empty one included, which means the cwd — must be
    // skipped whether or not its file exists.
    let probe_dir = PathBuf::from("c28-relative-path-probe");
    std::fs::create_dir_all(&probe_dir).expect("probe dir");
    std::fs::write(probe_dir.join("idea64.exe"), b"").expect("plant");
    std::fs::write("idea64.exe", b"").expect("plant");
    let found = detect(&probe(
        vec![probe_dir.clone(), PathBuf::new()],
        Vec::new(),
        Vec::new(),
        WINDOWS_PROBE,
        None,
    ));
    let _ = std::fs::remove_file(probe_dir.join("idea64.exe"));
    let _ = std::fs::remove_dir(&probe_dir);
    let _ = std::fs::remove_file("idea64.exe");

    assert!(
        !found.iter().any(|target| target.id == "idea"),
        "a relative PATH entry must never supply a launcher: {found:?}"
    );
}

#[test]
fn a_path_hit_resolves_to_its_canonical_executable() {
    let dir = TempDir::new().expect("temp");
    let tools = dir.path().join("tools");
    make_file(&tools.join("idea64.exe"));
    let injected = probe(
        vec![tools.clone()],
        Vec::new(),
        Vec::new(),
        WINDOWS_PROBE,
        None,
    );

    let found = detect(&injected);
    assert_eq!(
        found
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>(),
        ["idea"]
    );
    let resolved = resolve("idea", &injected).expect("installed");
    let expected = std::fs::canonicalize(tools.join("idea64.exe")).expect("canonical hit");
    match resolved {
        Resolved::Executable { program, .. } => assert_eq!(
            program, expected,
            "argv must carry the canonical identity, not the PATH spelling"
        ),
        other => panic!("expected an executable, got {other:?}"),
    }
}

#[test]
fn a_command_script_outside_the_install_layout_is_left_out() {
    let dir = TempDir::new().expect("temp");
    let base = dir.path();
    make_file(&base.join("Microsoft VS Code/Code.exe"));
    // A script whose product sits one level up but outside a `bin\`
    // directory: only the shim route can find this product, and the shim
    // route must refuse the layout.
    let stray = base.join("Microsoft VS Code/stray");
    make_file(&stray.join("code.cmd"));

    let found = resolve(
        "vscode",
        &probe(
            vec![stray],
            vec![("vscode", vec![])],
            Vec::new(),
            WINDOWS_PROBE,
            None,
        ),
    );

    assert!(
        found.is_none(),
        "a script outside the install layout is not a launcher: {found:?}"
    );
}

/// What this machine actually detects, read by hand:
/// `cargo test -p devboule installed_targets_on_this_machine -- --ignored
/// --nocapture`. Ignored because the answer is this machine's inventory,
/// not an assertion — every rule behind it is asserted above against an
/// injected probe.
#[test]
#[ignore]
fn installed_targets_on_this_machine() {
    for target in detect(&Probe::from_env()) {
        println!("{} — {} ({:?})", target.id, target.label, target.kind);
    }
}
