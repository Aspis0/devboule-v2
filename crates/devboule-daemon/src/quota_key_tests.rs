//! The key sources, against fixture files in a temp folder: never the person's
//! real `auth.json`, and no network.

use std::fs;
use std::path::PathBuf;

use super::{opencode_key, pi_auth_path, ApiKey, KeySources};

/// A fresh home folder under the temp dir, holding a fixture `auth.json` when
/// the test gives one.
struct Home(PathBuf);

impl Home {
    fn new(name: &str, auth: Option<&str>) -> Self {
        let root =
            std::env::temp_dir().join(format!("devboule-quota-key-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        if let Some(text) = auth {
            let path = pi_auth_path(&root);
            fs::create_dir_all(path.parent().expect("a parent")).expect("fixture dir");
            fs::write(path, text).expect("fixture auth");
        }
        Self(root)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn no_env(_: &str) -> Option<String> {
    None
}

fn env_key(value: &'static str) -> impl Fn(&str) -> Option<String> {
    move |name: &str| (name == "OPENCODE_API_KEY").then(|| value.to_string())
}

fn sources<'a>(env: &'a dyn Fn(&str) -> Option<String>, home: Option<&Home>) -> KeySources<'a> {
    KeySources {
        env,
        home: home.map(|home| home.0.clone()),
    }
}

fn text(key: &ApiKey) -> String {
    key.bearer().trim_start_matches("Bearer ").to_string()
}

#[test]
fn the_environment_variable_wins_over_the_file() {
    let home = Home::new(
        "env-wins",
        Some(r#"{"opencode":{"type":"api_key","key":"file-key"}}"#),
    );
    let env = env_key("env-key");
    let key = opencode_key(&sources(&env, Some(&home))).expect("a key");
    assert_eq!(text(&key), "env-key");
}

#[test]
fn a_blank_variable_falls_through_to_the_file() {
    let home = Home::new(
        "blank-env",
        Some(r#"{"opencode":{"type":"api_key","key":"file-key"}}"#),
    );
    let env = env_key("   ");
    let key = opencode_key(&sources(&env, Some(&home))).expect("a key");
    assert_eq!(text(&key), "file-key");
}

#[test]
fn the_file_entry_is_read_when_no_variable_is_set() {
    let home = Home::new(
        "file",
        Some(
            r#"{"anthropic":{"type":"api_key","key":"other"},"opencode":{"type":"api_key","key":"  file-key  "}}"#,
        ),
    );
    let key = opencode_key(&sources(&no_env, Some(&home))).expect("a key");
    assert_eq!(text(&key), "file-key");
}

#[test]
fn an_oauth_entry_is_not_a_key_for_this_source() {
    let home = Home::new(
        "oauth",
        Some(r#"{"opencode":{"type":"oauth","access":"token"}}"#),
    );
    assert!(opencode_key(&sources(&no_env, Some(&home))).is_none());
}

#[test]
fn a_command_key_is_not_run_and_so_is_no_key() {
    let home = Home::new(
        "command",
        Some(r#"{"opencode":{"type":"api_key","key":"!some-command"}}"#),
    );
    assert!(opencode_key(&sources(&no_env, Some(&home))).is_none());
}

#[test]
fn a_missing_file_or_a_malformed_one_is_no_key() {
    let missing = Home::new("missing", None);
    assert!(opencode_key(&sources(&no_env, Some(&missing))).is_none());
    let malformed = Home::new("malformed", Some("{not json"));
    assert!(opencode_key(&sources(&no_env, Some(&malformed))).is_none());
    let no_entry = Home::new(
        "no-entry",
        Some(r#"{"anthropic":{"type":"api_key","key":"k"}}"#),
    );
    assert!(opencode_key(&sources(&no_env, Some(&no_entry))).is_none());
}

#[test]
fn no_home_and_no_variable_is_no_key() {
    assert!(opencode_key(&sources(&no_env, None)).is_none());
}

#[test]
fn a_key_never_shows_its_text_in_debug_output() {
    let key = ApiKey::from_text("secret-text-value").expect("a key");
    let shown = format!("{key:?}");
    assert_eq!(shown, "ApiKey(redacted)");
    assert!(!shown.contains("secret-text-value"));
}
