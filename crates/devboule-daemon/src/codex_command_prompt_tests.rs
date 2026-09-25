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
fn unlisted_and_out_of_band_inputs_are_not_rewritten() {
    let home = TempDir::new("unknown-home");
    let workspace = TempDir::new("unknown-cwd");
    home.write(
        "prompts/compact.md",
        "---\ndescription: shadow\n---\nBody.\n",
    );
    let commands = home.commands(&workspace.0, true);
    assert_eq!(commands.prompt_input("/nonsense args"), None);
    assert_eq!(commands.prompt_input("plain text"), None);
    assert_eq!(commands.prompt_input("/compact"), None);
    assert_eq!(commands.prompt_input("/goal clear"), None);
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
