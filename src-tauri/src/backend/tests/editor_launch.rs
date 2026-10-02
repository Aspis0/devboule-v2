//! Launch plans for one resolved target: the argv every family is spelled
//! with, and the promise that no plan names a shell or a command script.

use std::path::{Path, PathBuf};

use crate::backend::editor_target_specs::Family;
use crate::backend::editor_targets::{launch_plan, Launch, Resolved, SpawnPlan};

fn spawn_plan(resolved: &Resolved) -> SpawnPlan {
    match launch_plan(
        resolved,
        Path::new("C:\\ws"),
        Path::new("C:\\ws\\src\\main.rs"),
        Some(7),
    ) {
        Launch::Spawn(plan) => plan,
        Launch::Reveal => panic!("an editor resolves to a spawn, not a reveal"),
    }
}

#[test]
fn a_launch_never_names_a_shell_or_a_command_script() {
    for (family, executable) in [
        (Family::Vscode, "C:\\tools\\Code.exe"),
        (Family::Zed, "C:\\tools\\zed.exe"),
        (Family::JetBrains, "C:\\tools\\idea64.exe"),
    ] {
        let plan = spawn_plan(&Resolved::Executable {
            program: PathBuf::from(executable),
            family,
        });
        assert_eq!(plan.program, PathBuf::from(executable));
        let program = plan.program.to_string_lossy().to_ascii_lowercase();
        assert!(
            !program.ends_with(".cmd"),
            "a script is never launched: {program}"
        );
        assert!(
            !program.ends_with(".bat"),
            "a script is never launched: {program}"
        );
        assert!(
            !program.ends_with("cmd.exe") && !program.ends_with("powershell.exe"),
            "no shell is ever launched: {program}"
        );
    }
    match launch_plan(
        &Resolved::FileManager,
        Path::new("C:\\ws"),
        Path::new("C:\\ws\\src\\main.rs"),
        None,
    ) {
        Launch::Reveal => {}
        Launch::Spawn(_) => panic!("the file manager reveals; it spawns nothing"),
    }
}

#[test]
fn the_argv_carries_the_plain_spelling_of_a_canonical_path() {
    let root = Path::new(r"\\?\C:\ws");
    let file = Path::new(r"\\?\C:\ws\src\main.rs");
    let vscode = Resolved::Executable {
        program: PathBuf::from(r"\\?\C:\tools\Code.exe"),
        family: Family::Vscode,
    };
    match launch_plan(&vscode, root, file, Some(7)) {
        Launch::Spawn(plan) => {
            assert_eq!(plan.args, [r"C:\ws", "--goto", r"C:\ws\src\main.rs:7"]);
        }
        other => panic!("expected a spawn, got an {other:?}"),
    }
    let unc_file = Path::new(r"\\?\UNC\srv\share\ws\a.rs");
    let idea = Resolved::Executable {
        program: PathBuf::from(r"C:\tools\idea64.exe"),
        family: Family::JetBrains,
    };
    match launch_plan(&idea, Path::new(r"\\?\UNC\srv\share\ws"), unc_file, None) {
        Launch::Spawn(plan) => assert_eq!(plan.args, [r"\\srv\share\ws\a.rs"]),
        other => panic!("expected a spawn, got an {other:?}"),
    }
}

#[test]
fn each_family_launches_with_its_own_line_form() {
    let root = Path::new("C:\\ws");
    let file = Path::new("C:\\ws\\src\\main.rs");

    let vscode = Resolved::Executable {
        program: PathBuf::from("C:\\tools\\Code.exe"),
        family: Family::Vscode,
    };
    match launch_plan(&vscode, root, file, Some(7)) {
        Launch::Spawn(plan) => {
            assert_eq!(plan.args, ["C:\\ws", "--goto", "C:\\ws\\src\\main.rs:7"]);
        }
        other => panic!("expected a spawn, got an {other:?}"),
    }
    match launch_plan(&vscode, root, file, None) {
        Launch::Spawn(plan) => {
            assert_eq!(plan.args, ["C:\\ws", "C:\\ws\\src\\main.rs"]);
        }
        other => panic!("expected a spawn, got an {other:?}"),
    }

    let zed = Resolved::Executable {
        program: PathBuf::from("C:\\tools\\zed.exe"),
        family: Family::Zed,
    };
    match launch_plan(&zed, root, file, Some(7)) {
        Launch::Spawn(plan) => {
            assert_eq!(plan.args, ["C:\\ws", "C:\\ws\\src\\main.rs:7"]);
        }
        other => panic!("expected a spawn, got an {other:?}"),
    }
    match launch_plan(&zed, root, file, None) {
        Launch::Spawn(plan) => {
            assert_eq!(plan.args, ["C:\\ws", "C:\\ws\\src\\main.rs"]);
        }
        other => panic!("expected a spawn, got an {other:?}"),
    }

    // The JetBrains CLI documents one path — `--line N <file>` or the file
    // alone — never a project root beside the file.
    let idea = Resolved::Executable {
        program: PathBuf::from("C:\\tools\\idea64.exe"),
        family: Family::JetBrains,
    };
    match launch_plan(&idea, root, file, Some(7)) {
        Launch::Spawn(plan) => {
            assert_eq!(plan.args, ["--line", "7", "C:\\ws\\src\\main.rs"]);
        }
        other => panic!("expected a spawn, got an {other:?}"),
    }
    match launch_plan(&idea, root, file, None) {
        Launch::Spawn(plan) => {
            assert_eq!(plan.args, ["C:\\ws\\src\\main.rs"]);
        }
        other => panic!("expected a spawn, got an {other:?}"),
    }

    // The bundle route names an app and cannot carry a line.
    match launch_plan(
        &Resolved::Bundle {
            name: "Zed".to_string(),
        },
        root,
        file,
        Some(7),
    ) {
        Launch::Spawn(plan) => {
            assert_eq!(plan.program, PathBuf::from("/usr/bin/open"));
            assert_eq!(plan.args, ["-a", "Zed", "C:\\ws", "C:\\ws\\src\\main.rs"]);
        }
        other => panic!("expected a spawn, got an {other:?}"),
    }
}
