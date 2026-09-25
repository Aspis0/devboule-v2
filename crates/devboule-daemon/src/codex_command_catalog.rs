//! What one Codex install offers as slash commands, read off the filesystem.
//!
//! The Codex app-server protocol publishes no command list — measured on our
//! own recorded handshake (`fixtures/wire/codex/E1-step1-handshake.jsonl`
//! carries `initialize`, `model/list` and `thread/start` answers and no command
//! surface) — so Paseo builds the list from the same two places on disk and
//! hard-codes the two commands the protocol does answer. This module is that
//! build, translated function by function from
//! `paseo-src/packages/server/src/server/agent/providers/codex-app-server-agent.ts`
//! (`listCodexCustomPrompts` :659-697, `listCodexSkills` :700-760,
//! `parseFrontMatter` :618-657, the built-in rows of `listCommands`
//! :4952-4968).
//!
//! [`command_table`] runs once at session start; its snapshot decides how a
//! picked command travels for that session.
//! A file that cannot be read is skipped, and no file content is ever logged —
//! the same rule the front-matter fallbacks below follow.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MAX_COMMAND_FILE_BYTES: u64 = 1024 * 1024;

pub(super) fn read_command_file(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_COMMAND_FILE_BYTES {
        return None;
    }
    use std::io::Read;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::fs::File::open(path)
        .ok()?
        .take(MAX_COMMAND_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_COMMAND_FILE_BYTES)
        .then(|| String::from_utf8_lossy(&bytes).into_owned())
}

use devboule_protocol::AvailableCommandView;

/// One command in the table, with what running it needs.
pub(crate) struct CommandEntry {
    pub(crate) name: String,
    description: String,
    hint: Option<String>,
    pub(crate) origin: CommandOrigin,
}

/// Where one entry came from, which is what decides the form its prompt takes.
pub(crate) enum CommandOrigin {
    /// Codex's own compaction request (Paseo's hardcoded `compact`, :4954).
    Compact,
    /// Codex's goal requests, offered only when the version gate passes
    /// (Paseo :4962-4968).
    Goal,
    /// A `~/.codex/prompts/<name>.md` custom prompt, carried by its path
    /// because running it reads the file again (Paseo :4030-4037).
    Prompt { path: PathBuf },
    /// A `.codex/skills/<dir>/SKILL.md` skill; `buildCommandPromptInput`
    /// includes both this path and the `$name args` text (:4044-4052).
    Skill { path: PathBuf },
}

/// The table Paseo's `listCommands` returns (:4937-4973): built-ins, then the
/// skills, then the prompts, sorted by name.
///
/// Paseo's own order for these sources is `builtin, appServerSkills,
/// fallbackSkills, prompts` and it sorts by `localeCompare`; a byte sort is
/// the same order for the lowercase names these directories hold in practice,
/// and the difference is menu order only.
pub(crate) fn command_table(
    codex_home: &Path,
    cwd: Option<&Path>,
    goals_enabled: bool,
) -> Vec<CommandEntry> {
    let mut entries = builtin_entries(goals_enabled);
    entries.extend(skill_entries(codex_home, cwd));
    entries.extend(prompt_entries(codex_home));
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

/// The table as the menu sees it: name, description and argument hint.
pub(crate) fn views(entries: &[CommandEntry]) -> Vec<AvailableCommandView> {
    entries
        .iter()
        .map(|entry| AvailableCommandView {
            name: entry.name.clone(),
            description: entry.description.clone(),
            hint: entry.hint.clone(),
        })
        .collect()
}

/// The two commands Codex's protocol answers that no file declares
/// (`listCommands` :4952-4968). `goal` is offered only when the version gate
/// passed — an older binary has no goal requests to answer.
fn builtin_entries(goals_enabled: bool) -> Vec<CommandEntry> {
    let mut entries = vec![CommandEntry {
        name: "compact".to_string(),
        description: "Summarize conversation to prevent hitting the context limit".to_string(),
        hint: None,
        origin: CommandOrigin::Compact,
    }];
    if goals_enabled {
        entries.push(CommandEntry {
            name: "goal".to_string(),
            description: "Set, pause, resume, or clear the agent's goal".to_string(),
            hint: Some("[<objective>|pause|resume|clear]".to_string()),
            origin: CommandOrigin::Goal,
        });
    }
    entries
}

/// `~/.codex/prompts/*.md`, one command each, named `prompts:<stem>`
/// (`listCodexCustomPrompts` :659-697).
///
/// The `prompts:` prefix is Paseo's, and it is what the front-end types back
/// into the composer; [`crate::codex_commands::parse_slash`] keeps the colon
/// inside the command name and rejects only a name with a `/` in it.
fn prompt_entries(codex_home: &Path) -> Vec<CommandEntry> {
    let prompts_dir = codex_home.join("prompts");
    let Ok(read_dir) = std::fs::read_dir(&prompts_dir) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for found in read_dir.flatten() {
        let file_type = match found.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        let Some(name) = found.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".md") else {
            continue;
        };
        // Paseo's filter (`entry.isFile() && name.endsWith(".md") && stem`):
        // a directory named `x.md` and a bare `.md` are not prompts.
        if !file_type.is_file() || stem.is_empty() {
            continue;
        }
        let path = found.path();
        // An unreadable prompt is skipped, never an error for the user — the
        // `try/catch` around `fs.readFile` at :674-679.
        let Some(content) = read_command_file(&path) else {
            continue;
        };
        let (front_matter, _) = front_matter(&content);
        entries.push(CommandEntry {
            name: format!("prompts:{stem}"),
            description: front_matter
                .get("description")
                .cloned()
                .unwrap_or_else(|| "Custom prompt".to_string()),
            // Both spellings Paseo accepts, in its order (:684-686).
            hint: front_matter
                .get("argument-hint")
                .or_else(|| front_matter.get("argument_hint"))
                .cloned()
                .filter(|hint| !hint.is_empty()),
            origin: CommandOrigin::Prompt { path },
        });
    }
    entries
}

/// The skill directories Paseo walks (`listCodexSkills` :700-760): the
/// workspace's `.codex/skills`, its parent's and the Git repository root's when
/// one resolves, then the user's own `~/.codex/skills`.
///
/// One level of directories each, and only those holding a `SKILL.md` — Paseo
/// reads `<skills dir>/<entry>/SKILL.md` and skips an entry whose read fails.
/// A skill needs both a `name` and a `description` in its front matter (:750-752)
/// and the first directory that names a skill wins (:753-759), which is why the
/// candidates are walked in order rather than collected.
fn skill_entries(codex_home: &Path, cwd: Option<&Path>) -> Vec<CommandEntry> {
    let mut candidates = Vec::new();
    if let Some(cwd) = cwd {
        candidates.push(cwd.join(".codex").join("skills"));
        if let Some(root) = git_root(cwd) {
            if let Some(parent) = cwd.parent() {
                candidates.push(parent.join(".codex").join("skills"));
            }
            candidates.push(root.join(".codex").join("skills"));
        }
    }
    candidates.push(codex_home.join("skills"));

    let mut by_name: BTreeMap<String, CommandEntry> = BTreeMap::new();
    for candidate in candidates {
        let Ok(read_dir) = std::fs::read_dir(&candidate) else {
            continue;
        };
        for found in read_dir.flatten() {
            let file_type = match found.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            // Paseo's filter (:726): a skill is a directory, or a link to one.
            if !(file_type.is_dir() || file_type.is_symlink()) {
                continue;
            }
            let Some(content) = read_command_file(&found.path().join("SKILL.md")) else {
                continue;
            };
            let (front_matter, _) = front_matter(&content);
            let (Some(name), Some(description)) =
                (front_matter.get("name"), front_matter.get("description"))
            else {
                continue;
            };
            by_name.entry(name.clone()).or_insert_with(|| CommandEntry {
                name: name.clone(),
                description: description.clone(),
                hint: None,
                origin: CommandOrigin::Skill {
                    path: found.path().join("SKILL.md"),
                },
            });
        }
    }
    by_name.into_values().collect()
}

/// `resolveCodexHomeDir` :548-550: `$CODEX_HOME`, else `~/.codex`.
///
/// Production resolves it once per session start and hands the path to
/// [`command_table`], so a test can point the whole surface at a temp directory
/// without touching the process environment.
#[cfg(not(test))]
pub(crate) fn resolve_home() -> PathBuf {
    if let Some(home) = std::env::var_os("CODEX_HOME") {
        let home = PathBuf::from(home);
        if !home.as_os_str().is_empty() {
            return home;
        }
    }
    // The two variables a shell sets: `USERPROFILE` on Windows, `HOME`
    // elsewhere.
    home_dir()
        .map(|home| home.join(".codex"))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

/// The two variables that name a home: `USERPROFILE` on Windows, `HOME`
/// elsewhere. There is no `dirs` crate in this workspace and no third guess:
/// a machine with neither answers a relative `.codex`, which reads nothing —
/// the skip, not a guess about where the home might be.
#[cfg(not(test))]
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Paseo's `workspaceGitService.resolveRepoRoot(cwd)`, answered the cheap way:
/// the nearest ancestor holding `.git`. `crates/devboule-daemon/src/git.rs`
/// resolves a root by running `git rev-parse --show-toplevel`, which would put
/// a process spawn on every session start for one candidate directory; the
/// walk answers the same question for the only use this has.
fn git_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The YAML-lite front matter Paseo parses (:618-657): a `---` fence at the
/// top, one `key: value` per line with the last one naming a key winning,
/// `#` lines skipped, and one quote at either end of the value dropped.
///
/// Returns the keys and the body after the closing fence — the body is what a
/// prompt command sends (see [`crate::codex_commands::prompt_body`]). The map
/// is ordered for reproducibility, which is all the callers need: they look
/// keys up, never by position.
pub(crate) fn front_matter(markdown: &str) -> (BTreeMap<String, String>, String) {
    let lines: Vec<&str> = markdown.split('\n').collect();
    let empty = BTreeMap::new();
    if lines.first().map(|line| line.trim()) != Some("---") {
        return (empty, markdown.to_string());
    }
    let Some(end) = lines.iter().skip(1).position(|line| line.trim() == "---") else {
        return (empty, markdown.to_string());
    };
    // `position` is relative to the `skip(1)`, so the fence line itself is at
    // index `end + 1` in the full list.
    let end = end + 1;
    let mut found = BTreeMap::new();
    for line in &lines[1..end] {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(index) = trimmed.find(':') else {
            continue;
        };
        if index == 0 {
            continue;
        }
        let key = trimmed[..index].trim();
        let value = trimmed[index + 1..].trim();
        // Paseo chains the two replaces (`:649-651`), so the second one sees
        // what the first left: a value quoted on one side only loses that side
        // and keeps the other, and a value quoted on both loses both.
        let value = value.strip_prefix(['\'', '"']).unwrap_or(value);
        let value = value.strip_suffix(['\'', '"']).unwrap_or(value);
        if !key.is_empty() && !value.is_empty() {
            // `frontMatter[key] = value` is an assignment: the LAST line naming
            // a key wins, which is not the rule the skill table below uses.
            found.insert(key.to_string(), value.to_string());
        }
    }
    (found, lines[end + 1..].join("\n"))
}

#[cfg(test)]
#[path = "codex_command_catalog_tests.rs"]
mod tests;
