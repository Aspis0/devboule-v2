//! The command list read off the filesystem: what a temp Codex home and a
//! temp workspace contribute, and what a version gate withholds.

use std::path::{Path, PathBuf};

use super::{command_table, front_matter, views};

/// A temp dir removed when the test ends, so a run leaves no prompt files
/// behind for the next one to find.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        Self(crate::test_dirs::test_temp_dir(&format!(
            "devboule-codex-{tag}"
        )))
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self
            .0
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the parent of a fixture file");
        }
        std::fs::write(&path, content).expect("a fixture file is written");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn names(codex_home: &Path, cwd: Option<&Path>, goals_enabled: bool) -> Vec<String> {
    command_table(codex_home, cwd, goals_enabled)
        .iter()
        .map(|entry| entry.name.clone())
        .collect()
}

#[test]
fn one_prompt_and_one_skill_are_listed_beside_compact() {
    // The brief's own case: a temp home holding one prompt file and one skill
    // yields those two plus `compact`, in name order.
    let home = TempDir::new("list-home");
    let workspace = TempDir::new("list-cwd");
    home.write(
        "prompts/commit.md",
        "---\ndescription: Draft a commit message\nargument-hint: [scope]\n---\nBody.\n",
    );
    workspace.write(
        ".codex/skills/plotting/SKILL.md",
        "---\nname: plotting\ndescription: Draw a chart from a csv\n---\nDo it.\n",
    );

    assert_eq!(
        names(&home.0, Some(&workspace.0), false),
        ["compact", "plotting", "prompts:commit"],
        "built-in first, then the two files, sorted by name"
    );
}

#[test]
fn the_prompt_keeps_the_front_matter_it_carries() {
    let home = TempDir::new("views-home");
    let workspace = TempDir::new("views-cwd");
    home.write(
        "prompts/review.md",
        "---\ndescription:  \"Quoted review\"  \nargument_hint: <branch>\n---\nBody.\n",
    );

    let found = views(&command_table(&home.0, Some(&workspace.0), false));
    let prompt = found
        .iter()
        .find(|view| view.name == "prompts:review")
        .expect("the prompt is listed");
    assert_eq!(
        prompt.description, "Quoted review",
        "the surrounding quotes are Paseo's to strip (:649-651)"
    );
    assert_eq!(
        prompt.hint.as_deref(),
        Some("<branch>"),
        "`argument_hint` is the second spelling Paseo accepts (:684-686)"
    );
}

#[test]
fn a_prompt_without_front_matter_is_still_listed() {
    // Paseo's fallback description, not a skip: the command exists whether or
    // not its author described it (:682).
    let home = TempDir::new("bare-home");
    let workspace = TempDir::new("bare-cwd");
    home.write("prompts/plain.md", "Just a body, no fence.\n");

    let found = views(&command_table(&home.0, Some(&workspace.0), false));
    let prompt = found
        .iter()
        .find(|view| view.name == "prompts:plain")
        .expect("an undescribed prompt is still a command");
    assert_eq!(prompt.description, "Custom prompt");
    assert_eq!(
        prompt.hint, None,
        "no argument hint is no hint, not an empty one"
    );
}

#[test]
fn goal_is_listed_only_when_the_gate_passed() {
    let home = TempDir::new("gate-home");
    let workspace = TempDir::new("gate-cwd");
    assert!(
        !names(&home.0, Some(&workspace.0), false).contains(&"goal".to_string()),
        "an older binary has no goal requests to answer, so the menu says nothing about goals"
    );
    let gated = names(&home.0, Some(&workspace.0), true);
    assert!(gated.contains(&"goal".to_string()), "{gated:?}");
    let found = views(&command_table(&home.0, Some(&workspace.0), true));
    let goal = found
        .iter()
        .find(|view| view.name == "goal")
        .expect("the gated goal entry");
    assert_eq!(
        goal.hint.as_deref(),
        Some("[<objective>|pause|resume|clear]"),
        "Paseo's own argument hint (:4965)"
    );
}

#[test]
fn a_file_named_exactly_dot_md_is_not_a_prompt() {
    // Paseo's filter keeps the stem: `entry.name.slice(0, -".md".length)` must
    // be non-empty (:671), so `.md` names no command.
    let home = TempDir::new("stem-home");
    let workspace = TempDir::new("stem-cwd");
    home.write(
        "prompts/.md",
        "---
description: nameless
---
Body.
",
    );
    assert_eq!(names(&home.0, Some(&workspace.0), false), ["compact"]);
}

#[test]
fn a_directory_named_like_a_prompt_is_not_one() {
    // Paseo's filter takes the entry type before the read (:668-672), so a
    // directory named `broken.md` never reaches the file reading below.
    let home = TempDir::new("dirprompt-home");
    let workspace = TempDir::new("dirprompt-cwd");
    std::fs::create_dir_all(home.0.join("prompts").join("broken.md")).expect("a directory");
    home.write(
        "prompts/readable.md",
        "---\ndescription: Fine\n---\nBody.\n",
    );

    assert_eq!(
        names(&home.0, Some(&workspace.0), false),
        ["compact", "prompts:readable"],
        "the entry that cannot be a prompt leaves nothing behind but its own absence"
    );
}

#[test]
#[cfg(windows)]
fn a_prompt_file_another_handle_holds_is_skipped_and_the_rest_still_lists() {
    // The read-failure half of the skip, on the platform this runs on: an
    // exclusive handle leaves a regular file that will not open, which is
    // Paseo's `try { readFile } catch { return null }` (:674-679).
    use std::os::windows::fs::OpenOptionsExt;
    let home = TempDir::new("held-home");
    let workspace = TempDir::new("held-cwd");
    let held = home.write("prompts/held.md", "---\ndescription: Hidden\n---\nBody.\n");
    home.write(
        "prompts/readable.md",
        "---\ndescription: Seen\n---\nBody.\n",
    );
    let handle = std::fs::OpenOptions::new()
        .read(true)
        // share_mode 0: nobody else may open it, reading included.
        .share_mode(0)
        .open(&held)
        .expect("the exclusive handle");

    let found = names(&home.0, Some(&workspace.0), false);
    assert!(
        found.contains(&"prompts:readable".to_string()),
        "one unreadable file does not take the directory down with it: {found:?}"
    );
    assert!(
        !found.contains(&"prompts:held".to_string()),
        "the file that would not open is skipped, not an error for the user: {found:?}"
    );
    drop(handle);
}

#[test]
#[cfg(unix)]
fn a_prompt_with_no_read_permission_is_skipped() {
    use std::os::unix::fs::PermissionsExt;
    let home = TempDir::new("mode-home");
    let workspace = TempDir::new("mode-cwd");
    let locked = home.write(
        "prompts/locked.md",
        "---\ndescription: Hidden\n---\nBody.\n",
    );
    let readable = home.write(
        "prompts/readable.md",
        "---\ndescription: Seen\n---\nBody.\n",
    );
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("mode 000");

    let found = names(&home.0, Some(&workspace.0), false);
    assert!(found.contains(&"prompts:readable".to_string()), "{found:?}");
    assert!(
        !found.contains(&"prompts:locked".to_string()),
        "an unreadable prompt is skipped, never an error: {found:?}"
    );
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o600));
}

#[test]
fn a_skill_needs_both_a_name_and_a_description() {
    // Paseo's `if (!name || !description) continue` (:750-752): a SKILL.md
    // missing either is not a command.
    let home = TempDir::new("skill-home");
    let workspace = TempDir::new("skill-cwd");
    workspace.write(
        ".codex/skills/nameless/SKILL.md",
        "---\ndescription: no name\n---\nx\n",
    );
    workspace.write(
        ".codex/skills/complete/SKILL.md",
        "---\nname: complete\ndescription: Both halves\n---\nx\n",
    );

    assert_eq!(
        names(&home.0, Some(&workspace.0), false),
        ["compact", "complete"]
    );
}

#[test]
fn the_nearest_skill_directory_wins_a_name_collision() {
    // First candidate wins (`commandsByName` is filled with `has`-guarded
    // inserts, :753-759) and the candidate order is cwd, then the home
    // (:712-716).
    let home = TempDir::new("clash-home");
    let workspace = TempDir::new("clash-cwd");
    home.write(
        "skills/dup/SKILL.md",
        "---\nname: dup\ndescription: From the home\n---\nx\n",
    );
    workspace.write(
        ".codex/skills/dup/SKILL.md",
        "---\nname: dup\ndescription: From the workspace\n---\nx\n",
    );

    let found = views(&command_table(&home.0, Some(&workspace.0), false));
    let dup = found
        .iter()
        .find(|view| view.name == "dup")
        .expect("the colliding skill");
    assert_eq!(
        dup.description, "From the workspace",
        "one entry per name, and the workspace is read first"
    );
}

#[test]
fn a_missing_prompts_directory_is_no_command_and_no_error() {
    let home = TempDir::new("empty-home");
    let workspace = TempDir::new("empty-cwd");
    assert_eq!(names(&home.0, Some(&workspace.0), false), ["compact"]);
    assert_eq!(names(&home.0, None, false), ["compact"]);
}

#[test]
fn front_matter_needs_both_fences_and_the_body_starts_after_them() {
    // Paseo :618-657: no leading `---`, or no closing one, means the whole
    // document is the body and there is no metadata.
    let (matter, body) = front_matter("---\nname: x\n---\nbody text\n");
    assert_eq!(matter.get("name").map(String::as_str), Some("x"));
    assert_eq!(body, "body text\n");

    let (matter, body) = front_matter("no fence here\nname: x\n");
    assert!(matter.is_empty());
    assert_eq!(body, "no fence here\nname: x\n");

    let (matter, body) = front_matter("---\nname: x\nunclosed\n");
    assert!(matter.is_empty(), "an unclosed fence is not metadata");
    assert_eq!(body, "---\nname: x\nunclosed\n");

    // A `#` line is a comment, and a repeated key keeps its LAST value: Paseo
    // assigns `frontMatter[key] = value` (:652) without guarding the write.
    let (matter, _) = front_matter("---\n# note\nname: first\nname: second\n---\n");
    assert_eq!(matter.get("name").map(String::as_str), Some("second"));

    // The two quote strips are independent (`:649-651`): a value with one side
    // quoted loses that side and keeps the other.
    let (matter, _) = front_matter("---\na: \"x\"\nb: \"x\nc: x\"\n---\n");
    assert_eq!(matter.get("a").map(String::as_str), Some("x"));
    assert_eq!(matter.get("b").map(String::as_str), Some("x"));
    assert_eq!(matter.get("c").map(String::as_str), Some("x"));
}
