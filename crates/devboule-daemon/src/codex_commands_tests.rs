//! The slash grammar, the request each command becomes, the line each answer
//! produces, and the form a picked command takes as a turn input.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::{parse_slash, Answer, CodexCommands, Command, Goal};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        Self(crate::test_dirs::test_temp_dir(&format!(
            "devboule-cmd-{tag}"
        )))
    }

    /// The commands of a session whose Codex home and workspace are these two
    /// temp directories.
    fn commands(&self, cwd: &Path, goals_enabled: bool) -> CodexCommands {
        CodexCommands::new(&self.0, Some(cwd), goals_enabled)
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

fn goal(commands: &CodexCommands, text: &str) -> Goal {
    match commands.command(text).expect("the text names goal") {
        Command::Goal(goal) => goal,
        Command::Compact => panic!("/compact is not a goal"),
    }
}

#[test]
fn the_slash_grammar_splits_the_name_from_its_arguments() {
    // `parseSlashCommandInput` :4986-5002.
    assert_eq!(parse_slash("/compact"), Some(("compact", None)));
    assert_eq!(
        parse_slash("  /goal   fix the bug  "),
        Some(("goal", Some("fix the bug"))),
        "the arguments are trimmed once and internal spaces survive"
    );
    assert_eq!(
        parse_slash("/prompts:commit src/a.ts"),
        Some(("prompts:commit", Some("src/a.ts")))
    );
    assert_eq!(parse_slash("/"), None, "a lone slash is not a command");
    assert_eq!(
        parse_slash("//x"),
        None,
        "a name holding a slash is not a command"
    );
    assert_eq!(
        parse_slash("do /thing"),
        None,
        "only a leading slash counts"
    );
    assert_eq!(parse_slash("/   "), None, "a name cannot be all whitespace");
}

#[test]
fn every_goal_form_becomes_the_request_paseo_sends() {
    let home = TempDir::new("req-home");
    let workspace = TempDir::new("req-cwd");
    let commands = home.commands(&workspace.0, true);

    let cases = [
        (
            "/goal ship it",
            "thread/goal/set",
            json!({"threadId": "t-1", "objective": "ship it", "status": "active"}),
        ),
        (
            "/goal PAUSE",
            "thread/goal/set",
            json!({"threadId": "t-1", "status": "paused"}),
        ),
        (
            "/goal resume",
            "thread/goal/set",
            json!({"threadId": "t-1", "status": "active"}),
        ),
        (
            "/goal clear",
            "thread/goal/clear",
            json!({"threadId": "t-1"}),
        ),
        (
            "/compact",
            "thread/compact/start",
            json!({"threadId": "t-1"}),
        ),
    ];
    for (text, method, params) in cases {
        let command = commands
            .command(text)
            .unwrap_or_else(|| panic!("{text} is an out-of-band command"));
        let (got_method, got_params) = command
            .request("t-1")
            .unwrap_or_else(|| panic!("{text} writes a request"));
        assert_eq!(got_method, method, "{text}");
        assert_eq!(got_params, params, "{text}");
    }
}

#[test]
fn pause_and_resume_carry_no_objective_and_set_carries_the_status_active() {
    // The three shapes differ by one field each, so each is pinned on its own:
    // Paseo's pause/resume params hold no `objective` at all (:5063-5074).
    let home = TempDir::new("fields-home");
    let workspace = TempDir::new("fields-cwd");
    let commands = home.commands(&workspace.0, true);

    let pause = commands
        .command("/goal pause")
        .expect("pause is a goal command");
    let (_, params) = pause.request("t-1").expect("pause writes a request");
    assert_eq!(
        params.get("objective"),
        None,
        "an empty objective would be an objective Codex never asked for"
    );
    let set = commands
        .command("/goal keep 9s")
        .expect("an objective sets the goal");
    let (_, params) = set.request("t-1").expect("set writes a request");
    assert_eq!(
        params.get("status").and_then(|v| v.as_str()),
        Some("active")
    );
    assert_eq!(
        params.get("objective").and_then(|v| v.as_str()),
        Some("keep 9s"),
        "the objective the human typed, not a trimmed stand-in"
    );
}

#[test]
fn a_bare_goal_answers_the_usage_line_without_touching_codex() {
    let home = TempDir::new("usage-home");
    let workspace = TempDir::new("usage-cwd");
    let commands = home.commands(&workspace.0, true);
    assert!(
        matches!(goal(&commands, "/goal"), Goal::Usage),
        "no arguments is the usage subcommand, not an empty objective"
    );
    assert!(
        Command::Goal(Goal::Usage).request("t-1").is_none(),
        "Paseo returns the line before it touches the client (:5035-5037)"
    );
    assert_eq!(
        Command::Goal(Goal::Usage).outcome(None).as_deref(),
        Some("Usage: /goal <objective>|pause|resume|clear")
    );
    assert_eq!(
        goal(&commands, "/goal Pause"),
        Goal::Pause,
        "the three words are matched case-insensitively (:199-203)"
    );
    assert!(
        matches!(goal(&commands, "/goal pause the goal"), Goal::Set { .. }),
        "a phrase that merely starts with pause is an objective"
    );
}

#[test]
fn each_answer_produces_the_line_paseo_shows() {
    let ok = [
        (
            Command::Goal(Goal::Set {
                objective: "ship it".to_string(),
            }),
            "Goal set: ship it",
        ),
        (Command::Goal(Goal::Pause), "Goal paused."),
        (Command::Goal(Goal::Resume), "Goal resumed."),
        (Command::Goal(Goal::Clear), "Goal cleared."),
    ];
    for (command, line) in ok {
        assert_eq!(command.outcome(None).as_deref(), Some(line));
    }
    assert_eq!(
        Command::Compact.outcome(None),
        None,
        "an accepted compaction says nothing: the app-server reports it with \
         `thread/compacted`, which the reader turns into its own notice"
    );
    assert_eq!(
        Command::Compact.outcome(Some("no such method")).as_deref(),
        Some("Failed to compact context: no such method")
    );
    assert_eq!(
        Command::Goal(Goal::Clear).outcome(Some("gone")).as_deref(),
        Some("Failed to update goal: gone")
    );
}

#[test]
fn an_older_binary_gets_neither_the_flag_nor_the_command() {
    // The gate is one decision with two halves: no `goal` in the list, and
    // `/goal x` is not a command either — so it leaves as the plain text the
    // human typed, which is Paseo's own fallback when the name is not in
    // `listCommands` (:4004-4021).
    let home = TempDir::new("old-home");
    let workspace = TempDir::new("old-cwd");
    let commands = home.commands(&workspace.0, false);
    assert!(
        commands.views().iter().all(|view| view.name != "goal"),
        "the menu withholds it"
    );
    assert_eq!(
        commands.command("/goal ship it"),
        None,
        "and nothing intercepts it"
    );
    assert_eq!(
        commands.prompt_input("/goal ship it"),
        None,
        "so the text is not rewritten either: it goes out verbatim"
    );
}

#[path = "codex_command_prompt_tests.rs"]
mod prompt_tests;

#[test]
fn an_answer_is_taken_once_and_by_its_own_id_only() {
    let home = TempDir::new("owed-home");
    let workspace = TempDir::new("owed-cwd");
    let commands = home.commands(&workspace.0, true);
    let command = commands
        .command("/goal clear")
        .expect("goal clear is a command");
    assert!(commands.owe("d-9", &command));
    assert!(
        matches!(commands.answer(&json!("d-other"), None), Answer::NotOurs),
        "an id that was never owed keeps the client's ordinary handling of its notice"
    );
    assert!(matches!(
        commands.answer(&json!("d-9"), None),
        Answer::Ours(line) if line.as_deref() == Some("Goal cleared.")
    ));
    assert!(
        matches!(commands.answer(&json!("d-9"), None), Answer::NotOurs),
        "one answer per request: the table gave it up"
    );
}

#[test]
fn an_accepted_compaction_owes_no_line_and_a_refused_one_owes_codex_words() {
    let home = TempDir::new("compact-home");
    let workspace = TempDir::new("compact-cwd");
    let commands = home.commands(&workspace.0, true);
    let command = commands.command("/compact").expect("compact is a command");
    commands.owe("d-1", &command);
    assert!(
        matches!(commands.answer(&json!("d-1"), None), Answer::Ours(None)),
        "our side says nothing; the notification says the rest"
    );
    commands.owe("d-2", &command);
    assert!(matches!(
        commands.answer(&json!("d-2"), Some("busy")),
        Answer::Ours(Some(line)) if line == "Failed to compact context: busy"
    ));
}

#[test]
fn a_full_owed_table_drops_the_notice_not_the_request() {
    let home = TempDir::new("cap-home");
    let workspace = TempDir::new("cap-cwd");
    let commands = home.commands(&workspace.0, true);
    let command = commands.command("/compact").expect("compact is a command");
    let filled = (0..48).all(|index| {
        // Past the cap the calls answer false; before it they must be true, so
        // the loop proves the table filled rather than refused from the start.
        index >= super::MAX_OWED_ANSWERS || commands.owe(&format!("d-{index}"), &command)
    });
    assert!(filled, "the table accepted up to its cap");
    assert!(
        !commands.owe("d-over", &command),
        "and refused to grow past it"
    );
    // The requests already owed still answer, so the cap costs only the newest
    // command its notice.
    assert!(matches!(
        commands.answer(&json!("d-0"), None),
        Answer::Ours(None)
    ));
}

#[test]
fn forgetting_a_request_stops_it_being_owed_an_answer() {
    let home = TempDir::new("forget-home");
    let workspace = TempDir::new("forget-cwd");
    let commands = home.commands(&workspace.0, true);
    let command = commands.command("/compact").expect("compact is a command");
    commands.owe("d-3", &command);
    commands.forget("d-3");
    assert!(
        matches!(commands.answer(&json!("d-3"), None), Answer::NotOurs),
        "a frame that never reached Codex has no answer to describe"
    );
}
