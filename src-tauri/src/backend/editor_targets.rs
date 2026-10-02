//! Nothing in this module opens a file through an OS file association.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;

use super::blocking::off_main_thread;
use super::editor_path_spelling::plain_spelling;
use super::editor_target_specs::{Family, TargetSpec, WinBase, SPECS};
use super::error::CommandError;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EditorTarget {
    pub id: String,
    pub label: String,
    pub kind: EditorTargetKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorTargetKind {
    Editor,
    FileManager,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Resolved {
    Executable {
        program: PathBuf,
        family: Family,
    },
    /// A macOS application bundle with no CLI on PATH: launched through
    /// `/usr/bin/open -a`, which carries no line.
    Bundle {
        name: String,
    },
    FileManager,
}

/// No shell anywhere in it: never a `.cmd`/`.bat` script, never `cmd.exe`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpawnPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
}

#[derive(Debug)]
pub(crate) enum Launch {
    Spawn(SpawnPlan),
    Reveal,
}

pub(crate) struct Probe {
    pub path: Vec<PathBuf>,
    pub known: Vec<(&'static str, Vec<PathBuf>)>,
    pub bundles: Vec<PathBuf>,
    pub suffixes: &'static [&'static str],
    pub file_manager: Option<&'static str>,
}

/// A PATH hit is a launcher when its suffix says executable; a `.cmd`/`.bat`
/// hit is a Windows command script, which is resolved to a real executable or
/// dropped — never run.
#[cfg(windows)]
pub(crate) const PATH_SUFFIXES: &[&str] = &[".exe", ".com", ".cmd", ".bat"];
#[cfg(not(windows))]
pub(crate) const PATH_SUFFIXES: &[&str] = &[""];

pub(crate) const FILE_MANAGER_ID: &str = "file-manager";

impl Probe {
    pub(crate) fn from_env() -> Probe {
        let path = std::env::var_os("PATH")
            .map(|raw| std::env::split_paths(&raw).collect::<Vec<_>>())
            .unwrap_or_default();
        let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
        let mut known = Vec::new();
        for spec in SPECS {
            let mut locations = Vec::new();
            for (base, relative) in spec.win {
                let base = match base {
                    WinBase::LocalAppData => local_app_data.as_ref(),
                    WinBase::ProgramFiles => program_files.as_ref(),
                };
                if let Some(base) = base {
                    locations.push(base.join(relative));
                }
            }
            known.push((spec.id, locations));
        }
        let mut bundles = ["/Applications", "/System/Applications"]
            .iter()
            .map(PathBuf::from)
            .filter(|dir| dir.is_dir())
            .collect::<Vec<_>>();
        if let Some(home) = std::env::var_os("HOME") {
            let applications = PathBuf::from(home).join("Applications");
            if applications.is_dir() {
                bundles.push(applications);
            }
        }
        Probe {
            path,
            known,
            bundles,
            suffixes: PATH_SUFFIXES,
            file_manager: file_manager_label(),
        }
    }

    fn known_of(&self, id: &str) -> &[PathBuf] {
        self.known
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, locations)| locations.as_slice())
            .unwrap_or(&[])
    }
}

fn file_manager_label() -> Option<&'static str> {
    if let Some(windir) = std::env::var_os("WINDIR") {
        if Path::new(&windir).join("explorer.exe").is_file() {
            return Some("Explorer");
        }
    }
    if Path::new("/System/Library/CoreServices/Finder.app").is_dir() {
        return Some("Finder");
    }
    None
}

/// Catalog order, the file manager always last.
pub(crate) fn detect(probe: &Probe) -> Vec<EditorTarget> {
    let mut found = SPECS
        .iter()
        .filter(|spec| resolve_spec(spec, probe).is_some())
        .map(|spec| EditorTarget {
            id: spec.id.to_string(),
            label: spec.label.to_string(),
            kind: EditorTargetKind::Editor,
        })
        .collect::<Vec<_>>();
    if let Some(label) = probe.file_manager {
        found.push(EditorTarget {
            id: FILE_MANAGER_ID.to_string(),
            label: label.to_string(),
            kind: EditorTargetKind::FileManager,
        });
    }
    found
}

/// `None` when this machine has no such target — including when its only
/// sign on PATH is a script outside the install layout.
pub(crate) fn resolve(id: &str, probe: &Probe) -> Option<Resolved> {
    if id == FILE_MANAGER_ID {
        return probe.file_manager.map(|_| Resolved::FileManager);
    }
    let spec = SPECS.iter().find(|spec| spec.id == id)?;
    resolve_spec(spec, probe)
}

fn resolve_spec(spec: &TargetSpec, probe: &Probe) -> Option<Resolved> {
    for candidate in probe.known_of(spec.id) {
        if candidate.is_file() {
            return executable(candidate.clone(), spec.family);
        }
    }
    for dir in &probe.path {
        // A relative entry — the empty one included, which means the cwd —
        // could point into the workspace being opened: only absolute PATH
        // entries may name a launcher.
        if dir.as_os_str().is_empty() || dir.is_relative() {
            continue;
        }
        for cli in spec.cli {
            for suffix in probe.suffixes {
                let hit = dir.join(format!("{cli}{suffix}"));
                if !hit.is_file() {
                    continue;
                }
                if matches!(*suffix, ".cmd" | ".bat") {
                    if let Some(program) = resolve_shim(spec, &hit) {
                        return executable(program, spec.family);
                    }
                    continue;
                }
                return executable(hit, spec.family);
            }
        }
    }
    for dir in &probe.bundles {
        for name in spec.mac {
            if dir.join(format!("{name}.app")).is_dir() {
                return Some(Resolved::Bundle {
                    name: (*name).to_string(),
                });
            }
        }
    }
    None
}

/// The identity argv carries: canonicalized the moment the file is found,
/// so the workspace check compares real paths rather than the spellings
/// PATH supplied.
fn executable(program: PathBuf, family: Family) -> Option<Resolved> {
    Some(Resolved::Executable {
        program: std::fs::canonicalize(program).ok()?,
        family,
    })
}

/// The install layout's own shape: the script in a `bin\` directory with
/// the product beside that directory (`...\Microsoft VS Code\bin\code.cmd`
/// over `...\Microsoft VS Code\Code.exe`). A script anywhere else — a
/// workspace on PATH, a stray copy — is not a launcher this catalog
/// trusts, so the target is left out.
fn resolve_shim(spec: &TargetSpec, shim: &Path) -> Option<PathBuf> {
    let bin = shim.parent()?;
    if !bin.file_name()?.eq_ignore_ascii_case("bin") {
        return None;
    }
    let product_dir = bin.parent()?;
    spec.win_exe
        .iter()
        .map(|name| product_dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The literal launch of one resolved target: the family's argv, the
/// bundle route's `/usr/bin/open -a`, or the file-manager reveal. Roots and
/// files arrive canonical from the caller and travel in their plain spelling.
pub(crate) fn launch_plan(
    resolved: &Resolved,
    root: &Path,
    file: &Path,
    line: Option<u32>,
) -> Launch {
    let root = plain_spelling(&root.to_string_lossy());
    let file = plain_spelling(&file.to_string_lossy());
    match resolved {
        Resolved::Executable { program, family } => {
            let args = match (family, line) {
                // The JetBrains CLI takes one path: a second positional path
                // would be another file to open, not the project this file
                // belongs to (`--line` is documented for a single file).
                (Family::JetBrains, Some(number)) => {
                    vec!["--line".to_string(), number.to_string(), file]
                }
                (Family::JetBrains, None) => vec![file],
                (Family::Vscode, Some(number)) => {
                    vec![root, "--goto".to_string(), format!("{file}:{number}")]
                }
                (Family::Zed, Some(number)) => vec![root, format!("{file}:{number}")],
                (_, None) => vec![root, file],
            };
            Launch::Spawn(SpawnPlan {
                program: program.clone(),
                args,
            })
        }
        // The bundle route names an app, not a line: `/usr/bin/open -a`
        // cannot carry one, so a bundle-only target opens the file at its
        // own first line.
        Resolved::Bundle { name } => Launch::Spawn(SpawnPlan {
            program: PathBuf::from("/usr/bin/open"),
            args: vec!["-a".to_string(), name.clone(), root, file],
        }),
        Resolved::FileManager => Launch::Reveal,
    }
}

pub(crate) fn spawn(plan: &SpawnPlan) -> std::io::Result<()> {
    let child = Command::new(&plan.program)
        .args(&plan.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Windows has no zombie state: dropping the handle leaves the editor running.
    #[cfg(windows)]
    drop(child);
    // One reaper thread per launch on unix — a launch is one click — so a
    // waited child never lingers as a zombie.
    #[cfg(unix)]
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
    Ok(())
}

/// The caret menu's list: the probe touches the disk, so it waits off the
/// window's thread like every other road that does.
#[tauri::command]
pub async fn editor_targets_list() -> Result<Vec<EditorTarget>, CommandError> {
    off_main_thread(|| -> Result<Vec<EditorTarget>, CommandError> {
        Ok(detect(&Probe::from_env()))
    })
    .await
}
