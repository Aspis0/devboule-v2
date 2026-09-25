//! Picked prompt and skill inputs, which stay turns and use Codex's input blocks.

use super::TempDir;

#[test]
fn a_picked_prompt_command_sends_its_expanded_body() {
    let home = TempDir::new("prompt-home");
    let workspace = TempDir::new("prompt-cwd");
    home.write(
        "prompts/commit.md",
        "---\ndescription: Draft it\n---\nOn $1: $ARGUMENTS\n",
    );
    let commands = home.commands(&workspace.0, false);
    assert_eq!(
        commands.prompt_input("/prompts:commit stage"),
        Some(serde_json::json!([{ "type": "text", "text": "On stage: stage\n" }])),
        "the prompt body keeps its trailing newline after expansion (:4030-4037)"
    );
    assert_eq!(
        commands.prompt_input("/prompts:commit"),
        Some(serde_json::json!([{ "type": "text", "text": "On : \n" }]))
    );
}

#[test]
fn a_first_prompt_composes_around_a_picked_command() {
    // The resolution runs on the user's message; the send path hands the
    // composed prefix alongside it, so no paragraph-guessing is involved.
    let home = TempDir::new("first-command-home");
    let workspace = TempDir::new("first-command-cwd");
    home.write(
        "prompts/commit.md",
        "---\ndescription: Draft it\n---\nDo $1\n",
    );
    let commands = home.commands(&workspace.0, false);
    let raw = "/prompts:commit release";
    let prefix = "standing instructions\n\nspawn prompt\n\nrecovered context";
    assert_eq!(
        commands.prompt_input_checked(raw, prefix),
        Ok(Some(serde_json::json!([
            { "type": "text", "text": "standing instructions\n\nspawn prompt\n\nrecovered context\n\nDo release\n" }
        ]))),
        "the first-turn prefix survives command expansion once"
    );
}

#[test]
fn a_picked_skill_command_sends_the_skill_and_text_blocks() {
    let home = TempDir::new("skill-home");
    let workspace = TempDir::new("skill-cwd");
    workspace.write(
        ".codex/skills/plotting/SKILL.md",
        "---\nname: plotting\ndescription: Draw it\n---\nSteps.\n",
    );
    let commands = home.commands(&workspace.0, false);
    let skill_path = workspace
        .0
        .join(".codex")
        .join("skills")
        .join("plotting")
        .join("SKILL.md");
    assert_eq!(
        commands.prompt_input("/plotting sales.csv"),
        Some(serde_json::json!([
            { "type": "skill", "name": "plotting", "path": skill_path },
            { "type": "text", "text": "$plotting sales.csv" },
        ])),
        "Paseo's populated-cache form includes both the skill block and fallback text (:4044-4052)"
    );
}

#[test]
fn unlisted_inputs_and_builtin_commands_are_not_rewritten() {
    let home = TempDir::new("unknown-home");
    let workspace = TempDir::new("unknown-cwd");
    let commands = home.commands(&workspace.0, true);
    assert_eq!(commands.prompt_input("/nonsense args"), None);
    assert_eq!(commands.prompt_input("plain text"), None);
    assert_eq!(commands.prompt_input("/compact"), None);
    assert_eq!(commands.prompt_input("/goal clear"), None);
}

#[test]
fn a_skill_named_goal_is_available_when_the_goal_builtin_is_gated_off() {
    let home = TempDir::new("skill-goal-home");
    let workspace = TempDir::new("skill-goal-cwd");
    workspace.write(
        ".codex/skills/goal/SKILL.md",
        "---\nname: goal\ndescription: Skill\n---\nBody.\n",
    );
    let commands = home.commands(&workspace.0, false);
    assert_eq!(
        commands.prompt_input("/goal ship it"),
        Some(serde_json::json!([
            { "type": "skill", "name": "goal", "path": workspace.0.join(".codex").join("skills").join("goal").join("SKILL.md") },
            { "type": "text", "text": "$goal ship it" },
        ])),
        "when the version gate is closed, Paseo's listed skill remains a picked command"
    );
}

#[test]
fn the_filesystem_fallback_ignores_the_enabled_front_matter_key() {
    let home = TempDir::new("disabled-skill-home");
    let workspace = TempDir::new("disabled-skill-cwd");
    workspace.write(
        ".codex/skills/hidden/SKILL.md",
        "---\nname: hidden\ndescription: Hidden\nenabled: false\n---\nBody.\n",
    );
    let commands = home.commands(&workspace.0, false);
    assert_eq!(
        commands.prompt_input("/hidden"),
        Some(serde_json::json!([
            { "type": "skill", "name": "hidden", "path": workspace.0.join(".codex").join("skills").join("hidden").join("SKILL.md") },
            { "type": "text", "text": "$hidden" }
        ]))
    );
}

#[test]
fn a_prompt_file_edited_after_the_session_starts_is_read_at_send_time() {
    let home = TempDir::new("fresh-home");
    let workspace = TempDir::new("fresh-cwd");
    home.write("prompts/live.md", "---\ndescription: v1\n---\nfirst\n");
    let commands = home.commands(&workspace.0, false);
    assert_eq!(
        commands.prompt_input("/prompts:live"),
        Some(serde_json::json!([{ "type": "text", "text": "first\n" }]))
    );
    home.write("prompts/live.md", "---\ndescription: v2\n---\nsecond\n");
    assert_eq!(
        commands.prompt_input("/prompts:live"),
        Some(serde_json::json!([{ "type": "text", "text": "second\n" }])),
        "the cached list holds the path, not the body (:4034-4036)"
    );
}

#[test]
fn the_session_start_snapshot_does_not_offer_new_prompts() {
    let home = TempDir::new("changing-home");
    let workspace = TempDir::new("changing-cwd");
    home.write("prompts/remove.md", "---\ndescription: Remove\n---\nbody\n");
    let commands = home.commands(&workspace.0, false);
    home.write("prompts/add.md", "---\ndescription: Add\n---\nnew\n");
    assert_eq!(
        commands.prompt_input_checked("/prompts:add", ""),
        Ok(None),
        "the send path uses the session-start catalogue snapshot"
    );
    std::fs::remove_file(home.0.join("prompts/remove.md")).expect("remove prompt fixture");
    assert!(commands.is_picked_command("/prompts:remove"));
    assert!(commands
        .prompt_input_checked("/prompts:remove", "")
        .expect_err("the session-start snapshot still selects its saved prompt")
        .contains("selected prompt file"));
}
