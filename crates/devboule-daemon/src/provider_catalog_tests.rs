//! Tests for the provider catalog: PATH resolution, user rows and availability.

use super::discover;
#[cfg(windows)]
use super::{
    executable_file_exists, launch_path_candidates_for_pathext, resolve_launch_command_in_paths,
    ResolvedLaunch,
};
use std::fs;
#[cfg(windows)]
use std::fs::File;
use std::path::{Path, PathBuf};

fn temporary_directory(label: &str) -> PathBuf {
    crate::test_dirs::test_temp_dir(&format!("devboule-provider-catalog-{label}"))
}

#[test]
fn command_name_candidate_shape_is_platform_specific() {
    let dir = PathBuf::from("provider-catalog-test");
    let candidates = super::launch_path_candidates_for_pathext(&dir, "agent", Some(".EXE;.CMD"));

    #[cfg(windows)]
    assert_eq!(
        candidates,
        vec![dir.join("agent.EXE"), dir.join("agent.CMD")]
    );
    #[cfg(not(windows))]
    assert_eq!(candidates, vec![dir.join("agent")]);
}

#[test]
fn external_versions_strip_controls_and_bidi_overrides() {
    assert_eq!(
        super::cap_external_version("1.2.3\nevil"),
        Some("1.2.3evil".to_string())
    );
    assert_eq!(
        super::cap_external_version("1.2.3\x1bevil"),
        Some("1.2.3evil".to_string())
    );
    assert_eq!(
        super::cap_external_version("1.2.3\u{202e}evil"),
        Some("1.2.3evil".to_string())
    );
    assert_eq!(
        super::cap_external_version("1.0.0-beta.1+終"),
        Some("1.0.0-beta.1+終".to_string())
    );
    assert_eq!(super::cap_external_version("\n\x1b\u{202e}"), None);
}

#[cfg(windows)]
#[test]
fn windows_fake_npm_shims_resolve_cmd_and_ps1() {
    let dir = temporary_directory("npm-shims");
    fs::create_dir_all(&dir).expect("temporary directory");
    File::create(dir.join("codex")).expect("fake POSIX shim");
    File::create(dir.join("codex.cmd")).expect("fake cmd shim");
    File::create(dir.join("qwen")).expect("fake POSIX shim");
    File::create(dir.join("qwen.ps1")).expect("fake powershell shim");
    File::create(dir.join("qwen.cmd")).expect("fake cmd shim");

    let paths = vec![dir.clone()];
    assert_eq!(
        resolve_launch_command_in_paths(&paths, "codex"),
        Some(ResolvedLaunch::program(super::normalize_windows_path(
            fs::canonicalize(dir.join("codex.cmd")).expect("canonical cmd shim"),
        )))
    );
    assert_eq!(
        resolve_launch_command_in_paths(&paths, "qwen"),
        Some(ResolvedLaunch::program(super::normalize_windows_path(
            fs::canonicalize(dir.join("qwen.cmd")).expect("canonical cmd shim"),
        )))
    );

    fs::remove_dir_all(dir).expect("temporary directory cleanup");
}

#[cfg(windows)]
fn npm_cmd_shim_contents(script_from_dp0: &str) -> String {
    format!(
            "\
@ECHO off
GOTO start
:find_dp0
SET dp0=%~dp0
EXIT /b
:start
SETLOCAL
CALL :find_dp0

IF EXIST \"%dp0%\\node.exe\" (
  SET \"_prog=%dp0%\\node.exe\"
) ELSE (
  SET \"_prog=node\"
  SET PATHEXT=%PATHEXT:;.JS;=;%
)

endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\{script_from_dp0}\" %*
"
        )
}

#[cfg(windows)]
fn canonical(path: &Path) -> PathBuf {
    super::normalize_windows_path(fs::canonicalize(path).expect("canonical path"))
}

#[cfg(windows)]
#[test]
fn windows_npm_cmd_shim_resolves_to_node_and_package_script() {
    let dir = temporary_directory("npm-real-shim");
    let node_dir = temporary_directory("npm-real-shim-node");
    fs::create_dir_all(
        dir.join("node_modules")
            .join("@openai")
            .join("codex")
            .join("bin"),
    )
    .expect("package directory");
    fs::create_dir_all(&node_dir).expect("node directory");
    File::create(dir.join("codex")).expect("fake POSIX shim");
    fs::write(
        dir.join("codex.cmd"),
        npm_cmd_shim_contents(r"node_modules\@openai\codex\bin\codex.js"),
    )
    .expect("npm cmd shim");
    fs::write(
        dir.join("node_modules")
            .join("@openai")
            .join("codex")
            .join("bin")
            .join("codex.js"),
        "console.log('codex');\n",
    )
    .expect("package script");
    File::create(node_dir.join("node.exe")).expect("fake node");

    let paths = vec![dir.clone(), node_dir.clone()];
    let resolved = resolve_launch_command_in_paths(&paths, "codex");
    assert_eq!(
        resolved,
        Some(ResolvedLaunch {
            program: canonical(&node_dir.join("node.exe")),
            prefix_args: vec![canonical(
                &dir.join("node_modules")
                    .join("@openai")
                    .join("codex")
                    .join("bin")
                    .join("codex.js"),
            )
            .to_string_lossy()
            .into_owned()],
        })
    );

    fs::remove_dir_all(dir).expect("temporary directory cleanup");
    fs::remove_dir_all(node_dir).expect("temporary node directory cleanup");
}

#[cfg(windows)]
#[test]
fn windows_npm_cmd_shim_prefers_sibling_node_exe() {
    let dir = temporary_directory("npm-local-node");
    fs::create_dir_all(dir.join("node_modules").join("pkg")).expect("package directory");
    File::create(dir.join("codex")).expect("fake POSIX shim");
    fs::write(
        dir.join("codex.cmd"),
        npm_cmd_shim_contents(r"node_modules\pkg\cli.js"),
    )
    .expect("npm cmd shim");
    fs::write(
        dir.join("node_modules").join("pkg").join("cli.js"),
        "/* js */\n",
    )
    .expect("package script");
    File::create(dir.join("node.exe")).expect("sibling node");

    let resolved = resolve_launch_command_in_paths(std::slice::from_ref(&dir), "codex");
    assert_eq!(
        resolved,
        Some(ResolvedLaunch {
            program: canonical(&dir.join("node.exe")),
            prefix_args: vec![
                canonical(&dir.join("node_modules").join("pkg").join("cli.js"))
                    .to_string_lossy()
                    .into_owned()
            ],
        })
    );

    fs::remove_dir_all(dir).expect("temporary directory cleanup");
}

#[cfg(windows)]
#[test]
fn windows_npm_cmd_shim_without_script_stays_the_cmd() {
    let dir = temporary_directory("npm-missing-script");
    let node_dir = temporary_directory("npm-missing-script-node");
    fs::create_dir_all(&dir).expect("temporary directory");
    fs::create_dir_all(&node_dir).expect("node directory");
    fs::write(
        dir.join("codex.cmd"),
        npm_cmd_shim_contents(r"node_modules\@openai\codex\bin\codex.js"),
    )
    .expect("npm cmd shim");
    File::create(node_dir.join("node.exe")).expect("fake node");

    assert_eq!(
        resolve_launch_command_in_paths(&[dir.clone(), node_dir.clone()], "codex"),
        Some(ResolvedLaunch::program(canonical(&dir.join("codex.cmd"))))
    );

    fs::remove_dir_all(dir).expect("temporary directory cleanup");
    fs::remove_dir_all(node_dir).expect("temporary node directory cleanup");
}

#[cfg(windows)]
#[test]
fn windows_npm_cmd_shim_without_node_stays_the_cmd() {
    let dir = temporary_directory("npm-missing-node");
    fs::create_dir_all(dir.join("node_modules").join("pkg")).expect("package directory");
    fs::write(
        dir.join("codex.cmd"),
        npm_cmd_shim_contents(r"node_modules\pkg\cli.js"),
    )
    .expect("npm cmd shim");
    fs::write(
        dir.join("node_modules").join("pkg").join("cli.js"),
        "/* js */\n",
    )
    .expect("package script");

    assert_eq!(
        resolve_launch_command_in_paths(std::slice::from_ref(&dir), "codex"),
        Some(ResolvedLaunch::program(canonical(&dir.join("codex.cmd"))))
    );

    fs::remove_dir_all(dir).expect("temporary directory cleanup");
}

#[cfg(windows)]
#[test]
fn windows_native_exe_is_not_unwrapped() {
    let dir = temporary_directory("native-exe");
    fs::create_dir_all(&dir).expect("temporary directory");
    File::create(dir.join("grok.exe")).expect("native grok");

    assert_eq!(
        resolve_launch_command_in_paths(std::slice::from_ref(&dir), "grok"),
        Some(ResolvedLaunch::program(canonical(&dir.join("grok.exe"))))
    );

    fs::remove_dir_all(dir).expect("temporary directory cleanup");
}

#[test]
fn npm_cmd_shim_parser_reads_measured_launch_lines() {
    let cases = [
        (
            r#"SET "_prog=node"
"%_prog%"  "%dp0%\node_modules\@openai\codex\bin\codex.js" %*"#,
            r"node_modules\@openai\codex\bin\codex.js",
        ),
        (
            r#"SET "_prog=node"
"%_prog%"  "%dp0%\node_modules\@earendil-works\pi-coding-agent\dist\bundle\cli.js" %*"#,
            r"node_modules\@earendil-works\pi-coding-agent\dist\bundle\cli.js",
        ),
        (
            r#"SET "_prog=node"
"%_prog%"  "%dp0%\node_modules\@qwen-code\qwen-code\cli-entry.js" %*"#,
            r"node_modules\@qwen-code\qwen-code\cli-entry.js",
        ),
        (
            r#"SET "_prog=node"
"%_prog%"  "%dp0%\node_modules\pnpm\bin\pnpm.cjs" %*"#,
            r"node_modules\pnpm\bin\pnpm.cjs",
        ),
    ];
    for (contents, expected) in cases {
        assert_eq!(
            super::npm_cmd_shim_script_relative(contents),
            Some(expected),
            "{contents}"
        );
    }
    assert_eq!(
        super::npm_cmd_shim_script_relative("@ECHO off\ncodex --version\n"),
        None
    );
}

#[test]
fn npm_launcher_shim_relative_extracts_script_from_real_npx_cmd() {
    // Verbatim tail of the real npx.cmd on this machine (npm 10.x).
    let contents = "\
SET \"NODE_EXE=%~dp0\\node.exe\"\n\
IF NOT EXIST \"%NODE_EXE%\" ( SET \"NODE_EXE=node\" )\n\
SET \"NPM_PREFIX_JS=%~dp0\\node_modules\\npm\\bin\\npm-prefix.js\"\n\
SET \"NPX_CLI_JS=%~dp0\\node_modules\\npm\\bin\\npx-cli.js\"\n\
FOR /F \"delims=\" %%F IN ('CALL \"%NODE_EXE%\" \"%NPM_PREFIX_JS%\"') DO ( SET \"NPM_PREFIX_NPX_CLI_JS=%%F\\node_modules\\npm\\bin\\npx-cli.js\" )\n\
IF EXIST \"%NPM_PREFIX_NPX_CLI_JS%\" ( SET \"NPX_CLI_JS=%NPM_PREFIX_NPX_CLI_JS%\" )\n\
\"%NODE_EXE%\" \"%NPX_CLI_JS%\" %*\n";
    assert_eq!(
        super::npm_launcher_shim_script_relative(contents),
        Some(r"node_modules\npm\bin\npx-cli.js")
    );
}

#[test]
fn npm_launcher_shim_relative_handles_forward_slashes_in_set() {
    let contents = "\
SET \"NODE_EXE=%~dp0/node.exe\"\n\
SET \"CLI_JS=%~dp0/node_modules/pkg/cli.js\"\n\
\"%NODE_EXE%\" \"%CLI_JS%\" %*\n";
    assert_eq!(
        super::npm_launcher_shim_script_relative(contents),
        Some("node_modules/pkg/cli.js")
    );
}

#[test]
fn npm_launcher_shim_relative_returns_none_when_node_exe_is_missing() {
    let contents = "\
SET \"CLI_JS=%~dp0\\cli.js\"\n\
\"%NODE_EXE%\" \"%CLI_JS%\" %*\n";
    assert_eq!(super::npm_launcher_shim_script_relative(contents), None);
}

#[test]
fn npm_launcher_shim_relative_returns_none_for_non_launcher_contents() {
    assert_eq!(
        super::npm_launcher_shim_script_relative("@ECHO off\necho hello\n"),
        None
    );
    // Per-package cmd-shim shape should NOT match the launcher parser.
    assert_eq!(
        super::npm_launcher_shim_script_relative(
            r#"SET "_prog=node"
"%_prog%"  "%dp0%\node_modules\pkg\cli.js" %*"#
        ),
        None
    );
}

#[cfg(windows)]
#[test]
fn npm_launcher_shim_rejects_dot_dot_traversal() {
    let dir = temporary_directory("launcher-dotdot");
    fs::create_dir_all(dir.join("node_modules").join("npm").join("bin")).expect("npm bin");
    File::create(dir.join("node.exe")).expect("node");
    fs::write(
        dir.join("npx.cmd"),
        "\
SET \"NODE_EXE=%~dp0\\node.exe\"\n\
SET \"NPX_CLI_JS=%~dp0\\..\\escape\\npx-cli.js\"\n\
\"%NODE_EXE%\" \"%NPX_CLI_JS%\" %*\n",
    )
    .expect("npx.cmd");
    fs::create_dir_all(dir.join("..").join("escape")).expect("escape dir");
    fs::write(
        dir.join("..").join("escape").join("npx-cli.js"),
        "/* npx */\n",
    )
    .expect("escape script");
    // The script exists but the relative path contains .. — must be None.
    assert_eq!(
        super::resolve_launch_command_in_paths(std::slice::from_ref(&dir), "npx"),
        Some(ResolvedLaunch::program(canonical(&dir.join("npx.cmd"))))
    );
    let _ = fs::remove_dir_all(dir);
}

#[cfg(windows)]
#[test]
fn windows_npm_launcher_shim_unwraps_to_node_and_script() {
    let dir = temporary_directory("launcher-unwrap");
    fs::create_dir_all(dir.join("node_modules").join("npm").join("bin")).expect("npm bin");
    File::create(dir.join("node.exe")).expect("node");
    // Real npx.cmd launcher shape.
    fs::write(
            dir.join("npx.cmd"),
            "\
SET \"NODE_EXE=%~dp0\\node.exe\"\n\
IF NOT EXIST \"%NODE_EXE%\" ( SET \"NODE_EXE=node\" )\n\
SET \"NPM_PREFIX_JS=%~dp0\\node_modules\\npm\\bin\\npm-prefix.js\"\n\
SET \"NPX_CLI_JS=%~dp0\\node_modules\\npm\\bin\\npx-cli.js\"\n\
FOR /F \"delims=\" %%F IN ('CALL \"%NODE_EXE%\" \"%NPM_PREFIX_JS%\"') DO ( SET \"NPM_PREFIX_NPX_CLI_JS=%%F\\node_modules\\npm\\bin\\npx-cli.js\" )\n\
IF EXIST \"%NPM_PREFIX_NPX_CLI_JS%\" ( SET \"NPX_CLI_JS=%NPM_PREFIX_NPX_CLI_JS%\" )\n\
\"%NODE_EXE%\" \"%NPX_CLI_JS%\" %*\n",
        )
        .expect("npx.cmd");
    fs::write(
        dir.join("node_modules")
            .join("npm")
            .join("bin")
            .join("npx-cli.js"),
        "/* npx */\n",
    )
    .expect("npx-cli.js");

    let resolved = resolve_launch_command_in_paths(std::slice::from_ref(&dir), "npx");
    assert_eq!(
        resolved,
        Some(ResolvedLaunch {
            program: canonical(&dir.join("node.exe")),
            prefix_args: vec![canonical(
                &dir.join("node_modules")
                    .join("npm")
                    .join("bin")
                    .join("npx-cli.js"),
            )
            .to_string_lossy()
            .into_owned()],
        })
    );

    let _ = fs::remove_dir_all(dir);
}

#[cfg(windows)]
#[test]
fn windows_launch_candidates_follow_pathext_and_skip_the_naked_name() {
    let dir = temporary_directory("pathext");
    fs::create_dir_all(&dir).expect("temporary directory");

    assert_eq!(
        launch_path_candidates_for_pathext(&dir, "agent", Some(".BAT;.CMD;.EXE")),
        vec![
            dir.join("agent.BAT"),
            dir.join("agent.CMD"),
            dir.join("agent.EXE"),
        ]
    );
    assert_eq!(
        launch_path_candidates_for_pathext(&dir, "agent.cmd", Some(".BAT;.CMD;.EXE")),
        vec![dir.join("agent.cmd")]
    );

    fs::remove_dir_all(dir).expect("temporary directory cleanup");
}

#[cfg(windows)]
fn existing_launch_candidates(dir: &Path, command: &str, pathext: &str) -> Vec<PathBuf> {
    launch_path_candidates_for_pathext(dir, command, Some(pathext))
        .into_iter()
        .filter(|path| executable_file_exists(path))
        .collect()
}

#[cfg(windows)]
#[test]
fn windows_only_powershell_shim_is_not_launchable() {
    let dir = temporary_directory("ps1-only");
    fs::create_dir_all(&dir).expect("temporary directory");
    File::create(dir.join("agent.ps1")).expect("fake powershell shim");

    assert!(existing_launch_candidates(&dir, "agent", ".PS1").is_empty());

    fs::remove_dir_all(dir).expect("temporary directory cleanup");
}

#[cfg(windows)]
#[test]
fn windows_cmd_wins_over_powershell_even_when_pathext_prefers_ps1() {
    let dir = temporary_directory("ps1-before-cmd");
    fs::create_dir_all(&dir).expect("temporary directory");
    File::create(dir.join("agent.ps1")).expect("fake powershell shim");
    File::create(dir.join("agent.cmd")).expect("fake cmd shim");

    assert_eq!(
        existing_launch_candidates(&dir, "agent", ".PS1;.CMD"),
        vec![dir.join("agent.CMD")]
    );

    fs::remove_dir_all(dir).expect("temporary directory cleanup");
}

#[cfg(windows)]
#[test]
fn windows_ignores_pathext_entries_without_a_direct_launcher() {
    let dir = temporary_directory("unsupported-pathext");
    fs::create_dir_all(&dir).expect("temporary directory");
    File::create(dir.join("agent.VBS")).expect("fake visual basic script");
    File::create(dir.join("agent.JS")).expect("fake javascript script");

    assert!(existing_launch_candidates(&dir, "agent", ".VBS;.JS").is_empty());

    fs::remove_dir_all(dir).expect("temporary directory cleanup");
}

#[cfg(windows)]
#[test]
fn windows_pathext_fallback_handles_empty_space_and_malformed_entries() {
    let dir = temporary_directory("pathext-fallback");
    fs::create_dir_all(&dir).expect("temporary directory");

    let default_candidates = vec![
        dir.join("agent.COM"),
        dir.join("agent.EXE"),
        dir.join("agent.BAT"),
        dir.join("agent.CMD"),
    ];
    for pathext in [None, Some(""), Some("   "), Some(".VBS;.JS")] {
        let expected = if pathext == Some(".VBS;.JS") {
            Vec::new()
        } else {
            default_candidates.clone()
        };
        assert_eq!(
            launch_path_candidates_for_pathext(&dir, "agent", pathext),
            expected
        );
    }
    assert_eq!(
        launch_path_candidates_for_pathext(&dir, "agent", Some("eXe;cMd;.PS1")),
        vec![dir.join("agent.eXe"), dir.join("agent.cMd")]
    );

    fs::remove_dir_all(dir).expect("temporary directory cleanup");
}

#[cfg(windows)]
#[test]
fn windows_resolved_paths_drop_verbatim_prefix_without_breaking_unc() {
    assert_eq!(
        super::normalize_windows_path(PathBuf::from(r"\\?\C:\Users\Name\agent.exe")),
        PathBuf::from(r"C:\Users\Name\agent.exe")
    );
    assert_eq!(
        super::normalize_windows_path(PathBuf::from(r"\\?\UNC\server\share\agent.exe")),
        PathBuf::from(r"\\server\share\agent.exe")
    );
}

fn launch_display(agent: &super::InstalledAgent) -> String {
    let mut line = agent.executable.display().to_string();
    for arg in &agent.prefix_args {
        line.push(' ');
        line.push_str(arg);
    }
    line
}

fn fake_cli_path(dir: &Path, name: &str) {
    #[cfg(windows)]
    File::create(dir.join(format!("{name}.exe"))).expect("fake cli");
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, []).expect("fake cli");
        let mut perms = std::fs::metadata(&path).expect("meta").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod");
    }
}

#[test]
fn chat_protocol_reports_stream_json_acp_pi_rpc_or_none() {
    let dir = temporary_directory("chat-protocol");
    std::fs::create_dir_all(&dir).expect("temporary directory");
    fake_cli_path(&dir, "claude");
    fake_cli_path(&dir, "grok");
    fake_cli_path(&dir, "codex");
    fake_cli_path(&dir, "pi");
    let discovered = super::discover_in_paths(std::slice::from_ref(&dir));
    let protocol_of = |id: &str| {
        discovered
            .agents
            .iter()
            .find(|agent| agent.id == id)
            .and_then(super::chat_protocol)
    };
    assert_eq!(protocol_of("claude"), Some("stream-json"));
    assert_eq!(protocol_of("grok"), Some("acp"));
    assert_eq!(protocol_of("codex"), Some("codex-app-server"));
    assert_eq!(protocol_of("pi"), Some("pi-rpc"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn protocol_argv_inserts_prefix_between_executable_and_flags() {
    let exe = PathBuf::from(r"C:\nvm\node.exe");
    let prefix = vec![r"C:\npm\claude.js".to_string()];
    let argv = super::protocol_argv(&exe, &prefix, &["-p", "--verbose"]);
    assert_eq!(
        argv,
        vec![
            r"C:\nvm\node.exe".to_string(),
            r"C:\npm\claude.js".to_string(),
            "-p".to_string(),
            "--verbose".to_string(),
        ]
    );
}

#[test]
fn protocol_command_element_zero_is_the_resolved_executable() {
    let dir = temporary_directory("honest-argv");
    #[cfg(windows)]
    {
        fs::create_dir_all(&dir).expect("temporary directory");
        File::create(dir.join("claude.exe")).expect("fake claude");
        File::create(dir.join("grok.exe")).expect("fake grok");
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(&dir).expect("temporary directory");
        std::fs::write(dir.join("claude"), []).expect("fake claude");
        std::fs::write(dir.join("grok"), []).expect("fake grok");
        for name in ["claude", "grok"] {
            let path = dir.join(name);
            let mut perms = std::fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).expect("chmod");
        }
    }
    let discovered = super::discover_in_paths(std::slice::from_ref(&dir));
    let claude = discovered
        .agents
        .iter()
        .find(|agent| agent.id == "claude")
        .expect("claude on fake PATH");
    let stream = claude
        .stream_json_command
        .as_ref()
        .expect("claude speaks stream-json");
    assert_eq!(
        stream[0],
        claude.executable.to_string_lossy(),
        "stream_json_command[0] must be the resolved executable, not the catalog id"
    );
    let mut expected = vec![claude.executable.to_string_lossy().into_owned()];
    expected.extend(claude.prefix_args.iter().cloned());
    expected.extend(
        super::CLAUDE_STREAM_JSON_ARGS
            .iter()
            .map(|arg| (*arg).to_string()),
    );
    assert_eq!(
        stream, &expected,
        "spawned stream-json argv must stay executable + prefix + measured flags"
    );

    let grok = discovered
        .agents
        .iter()
        .find(|agent| agent.id == "grok")
        .expect("grok on fake PATH");
    let acp = grok.acp_command.as_ref().expect("grok speaks ACP");
    assert_eq!(
        acp[0],
        grok.executable.to_string_lossy(),
        "acp_command[0] must be the resolved executable, not the catalog id"
    );
    let mut expected = vec![grok.executable.to_string_lossy().into_owned()];
    expected.extend(grok.prefix_args.iter().cloned());
    expected.extend(["agent", "stdio"].iter().map(|arg| (*arg).to_string()));
    assert_eq!(
        acp, &expected,
        "spawned ACP argv must stay executable + prefix + acp args"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn npm_package_version_walk_stays_inside_node_modules() {
    let dir = temporary_directory("package-version-walk");
    let script = dir
        .join("node_modules")
        .join("@scope")
        .join("pkg")
        .join("dist")
        .join("cli.js");
    fs::create_dir_all(script.parent().expect("script parent")).expect("fixture dirs");
    fs::write(
        script
            .parent()
            .expect("dist")
            .parent()
            .expect("package")
            .join("package.json"),
        r#"{"version":"3.4.5"}"#,
    )
    .expect("package json");
    assert_eq!(
        super::package_json_version_from_script(&script),
        Some("3.4.5".to_string())
    );

    fs::write(dir.join("package.json"), r#"{"version":"9.9.9"}"#).expect("outer package");
    let no_inner_version = dir
        .join("node_modules")
        .join("other")
        .join("dist")
        .join("cli.js");
    fs::create_dir_all(no_inner_version.parent().expect("other parent")).expect("other dirs");
    assert_eq!(
        super::package_json_version_from_script(&no_inner_version),
        None,
        "the walk must reject leaving node_modules"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
#[ignore = "measurement, not an assertion; run by hand with --ignored --nocapture"]
fn reports_installed_cli_agents() {
    let agents = discover().agents;
    println!("provider catalog found {} agent(s):", agents.len());
    for agent in agents {
        println!(
            "{} => {} | ACP={:?} | auth={:?}",
            agent.id,
            launch_display(&agent),
            agent.acp_command,
            agent.authentication
        );
    }
    if let Some(agent) = super::first_acp_available() {
        println!(
            "default ACP: {} => {}",
            agent.id,
            agent
                .acp_command
                .expect("selected agent offers ACP")
                .join(" ")
        );
    } else {
        println!("default ACP: none");
    }
}

#[test]
#[ignore = "measurement, not an assertion; run by hand with --ignored --nocapture"]
fn reports_cli_launchability() {
    for agent in discover().agents {
        match super::probe_version(&agent) {
            Ok(output) => println!(
                "{} => STARTED exit={:?} stdout={:?} stderr={:?}",
                agent.id,
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
            Err(error) => println!(
                "{} => NOT STARTED path={} error={error}",
                agent.id,
                launch_display(&agent)
            ),
        }
    }
}

struct UnreadableDirectory {
    path: PathBuf,
    #[cfg(windows)]
    user: String,
    #[cfg(unix)]
    original_mode: std::fs::Permissions,
}

impl UnreadableDirectory {
    fn new(path: &Path) -> Self {
        #[cfg(windows)]
        {
            use std::ffi::OsStr;
            let user = String::from_utf8(
                std::process::Command::new("whoami")
                    .output()
                    .expect("whoami")
                    .stdout,
            )
            .expect("whoami output")
            .trim()
            .to_string();
            let deny = format!("{user}:(OI)(CI)(RX)");
            let result = std::process::Command::new("icacls")
                .args([path.as_os_str(), OsStr::new("/deny"), OsStr::new(&deny)])
                .status()
                .expect("icacls");
            assert!(result.success(), "icacls failed to deny directory access");
            Self {
                path: path.to_path_buf(),
                user,
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::metadata(path).expect("directory metadata");
            let original_mode = metadata.permissions();
            let mut denied = original_mode.clone();
            denied.set_mode(0);
            std::fs::set_permissions(path, denied).expect("remove directory permissions");
            Self {
                path: path.to_path_buf(),
                original_mode,
            }
        }
    }
}

impl Drop for UnreadableDirectory {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use std::ffi::OsStr;
            let _ = std::process::Command::new("icacls")
                .args([
                    self.path.as_os_str(),
                    OsStr::new("/remove:d"),
                    OsStr::new(&self.user),
                ])
                .status();
        }
        #[cfg(unix)]
        {
            let _ = std::fs::set_permissions(&self.path, self.original_mode.clone());
        }
    }
}

#[test]
fn discover_counts_unreadable_path_directories() {
    let readable = temporary_directory("readable");
    std::fs::create_dir_all(&readable).expect("readable");
    let blocked = temporary_directory("blocked");
    std::fs::create_dir_all(&blocked).expect("blocked");
    let guard = UnreadableDirectory::new(&blocked);
    let discovery = super::discover_in_paths(&[readable.clone(), blocked.clone()]);
    let unreadable = discovery.unreadable_dirs;
    drop(guard);
    let _ = std::fs::remove_dir_all(&readable);
    let _ = std::fs::remove_dir_all(&blocked);
    assert_eq!(
        unreadable, 1,
        "an unreadable PATH directory must not be reported as missing"
    );
}

struct FixtureFetch;

impl crate::registry::RegistryFetch for FixtureFetch {
    fn fetch_body(&self) -> Result<String, String> {
        Ok(crate::registry::TEST_REGISTRY_FIXTURE.to_string())
    }
}

struct CoverageFixtureFetch;

impl crate::registry::RegistryFetch for CoverageFixtureFetch {
    fn fetch_body(&self) -> Result<String, String> {
        Ok(r#"{
  "agents": [
    {
      "id": "claude-acp",
      "distribution": {
        "npx": { "package": "claude-acp@1.0.0" }
      }
    },
    {
      "id": "codex-acp",
      "distribution": {
        "npx": {
          "package": "codex-acp@1.0.0",
          "args": ["--registry=https://evil"]
        }
      }
    },
    {
      "id": "pi-acp",
      "distribution": {
        "npx": { "package": "pi-acp@1.0.0" }
      }
    }
  ]
}"#
        .to_string())
    }
}

#[cfg(feature = "server")]
#[test]
fn missing_known_npm_rows_are_settings_only_and_never_session_resolvable() {
    let cache = temporary_directory("synthetic-npm-cache");
    let empty_path = temporary_directory("synthetic-npm-path");
    fs::create_dir_all(&empty_path).expect("empty PATH directory");
    let catalog =
        super::discover_catalog_in_paths(&FixtureFetch, &cache, std::slice::from_ref(&empty_path));
    let codex = catalog
        .agents
        .iter()
        .find(|agent| agent.id == "codex")
        .expect("known codex npm row");
    assert!(!codex.installed);
    assert!(codex.executable.as_os_str().is_empty());
    assert!(codex.acp_command.is_none());
    assert_eq!(super::chat_protocol(codex), None);
    assert_eq!(
        codex.pickable,
        Some(false),
        "the synthetic not-installed codex row is never a chat-picker entry"
    );
    assert_eq!(codex.install_channel, super::InstallChannel::Npm);
    assert_eq!(codex.npm_package, Some("@openai/codex"));
    assert!(
        super::find_available_in_paths("codex", std::slice::from_ref(&empty_path)).is_none(),
        "find_available must not return a synthetic row"
    );
    assert!(
        super::find_in_catalog_in_paths(
            "codex",
            &FixtureFetch,
            &cache,
            std::slice::from_ref(&empty_path)
        )
        .is_none(),
        "find_in_catalog must not return a synthetic row to session creation"
    );
    let _ = fs::remove_dir_all(cache);
    let _ = fs::remove_dir_all(empty_path);
}

#[cfg(feature = "server")]
#[test]
fn registry_coverage_marks_chat_wrappers_unpickable_when_native_exists() {
    let dir = temporary_directory("registry-coverage-native");
    fs::create_dir_all(&dir).expect("temporary directory");
    fake_cli_path(&dir, "claude");
    fake_cli_path(&dir, "codex");
    fake_cli_path(&dir, "pi");
    fake_cli_path(&dir, "npx");
    let cache = temporary_directory("registry-coverage-native-cache");
    let catalog =
        super::discover_catalog_in_paths(&CoverageFixtureFetch, &cache, std::slice::from_ref(&dir));

    let claude = catalog
        .agents
        .iter()
        .find(|agent| agent.id == "claude-acp")
        .expect("claude-acp from registry");
    assert_eq!(
        claude.pickable,
        Some(false),
        "native Claude is covered by stream-json, so claude-acp remains Settings-only"
    );
    assert_eq!(claude.launch_args, Some(Vec::new()));
    assert_eq!(
        catalog
            .agents
            .iter()
            .find(|agent| agent.id == "codex-acp")
            .expect("codex-acp from registry")
            .launch_args,
        Some(vec!["--registry=https://evil".to_string()])
    );
    assert_eq!(
        catalog
            .agents
            .iter()
            .find(|agent| agent.id == "codex-acp")
            .expect("codex-acp from registry")
            .pickable,
        Some(false),
        "native Codex uses app-server, so codex-acp remains Settings-only"
    );
    assert_eq!(
            catalog
                .agents
                .iter()
                .find(|agent| agent.id == "pi-acp")
                .expect("pi-acp from registry")
                .pickable,
            Some(false),
            "native pi is pickable through pi-rpc; pi-acp lacks native-tool permission requests and remains Settings-only"
        );

    let no_native_dir = temporary_directory("registry-coverage-no-native");
    fs::create_dir_all(&no_native_dir).expect("temporary directory");
    fake_cli_path(&no_native_dir, "npx");
    let no_native_cache = temporary_directory("registry-coverage-no-native-cache");
    let no_native = super::discover_catalog_in_paths(
        &CoverageFixtureFetch,
        &no_native_cache,
        std::slice::from_ref(&no_native_dir),
    );
    assert_eq!(
        no_native
            .agents
            .iter()
            .find(|agent| agent.id == "claude-acp")
            .expect("claude-acp without native claude")
            .pickable,
        None,
        "without native Claude, the claude-acp wrapper remains pickable"
    );

    let _ = fs::remove_dir_all(dir);
    let _ = fs::remove_dir_all(cache);
    let _ = fs::remove_dir_all(no_native_dir);
    let _ = fs::remove_dir_all(no_native_cache);
}

#[cfg(feature = "server")]
#[test]
fn native_installed_grok_drops_registry_grok_build() {
    let dir = temporary_directory("native-beats-registry");
    fs::create_dir_all(&dir).expect("temporary directory");
    fake_cli_path(&dir, "grok");
    let cache = temporary_directory("native-beats-cache");
    fs::create_dir_all(&cache).expect("cache");
    let catalog =
        super::discover_catalog_in_paths(&FixtureFetch, &cache, std::slice::from_ref(&dir));
    let ids: Vec<&str> = catalog
        .agents
        .iter()
        .map(|agent| agent.id.as_str())
        .collect();
    assert!(ids.contains(&"grok"), "native grok must remain: {ids:?}");
    assert!(
        !ids.contains(&"grok-build"),
        "registry grok-build must not appear next to native grok: {ids:?}"
    );
    assert!(
        ids.contains(&"codex-acp"),
        "codex-acp remains visible when native Codex is absent: {ids:?}"
    );
    let grok = catalog
        .agents
        .iter()
        .find(|agent| agent.id == "grok")
        .expect("grok");
    assert_eq!(grok.origin, super::ProviderOrigin::UserBinary);
    let codex = catalog
        .agents
        .iter()
        .find(|agent| agent.id == "codex-acp")
        .expect("codex-acp");
    assert_eq!(codex.origin, super::ProviderOrigin::NpxWrapper);
    let _ = fs::remove_dir_all(dir);
    let _ = fs::remove_dir_all(cache);
}

#[cfg(all(windows, feature = "server"))]
#[test]
fn npx_wrapper_argv_is_node_and_npx_cli_not_cmd() {
    let dir = temporary_directory("npx-unwrap");
    fs::create_dir_all(dir.join("node_modules").join("npm").join("bin")).expect("npm bin");
    File::create(dir.join("node.exe")).expect("node");
    fs::write(
        dir.join("npx.cmd"),
        npm_cmd_shim_contents(r"node_modules\npm\bin\npx-cli.js"),
    )
    .expect("npx shim");
    fs::write(
        dir.join("node_modules")
            .join("npm")
            .join("bin")
            .join("npx-cli.js"),
        "/* npx */\n",
    )
    .expect("npx-cli.js");
    let cache = temporary_directory("npx-unwrap-cache");
    fs::create_dir_all(&cache).expect("cache");
    let catalog =
        super::discover_catalog_in_paths(&FixtureFetch, &cache, std::slice::from_ref(&dir));
    let codex = catalog
        .agents
        .iter()
        .find(|agent| agent.id == "codex-acp")
        .expect("codex-acp from registry");
    let argv = codex
        .acp_command
        .as_ref()
        .expect("npx-wrapper must resolve an ACP command");
    assert!(
        !argv[0].to_ascii_lowercase().contains(".cmd"),
        "argv[0] must be node.exe, not a .cmd shim: {}",
        argv[0]
    );
    assert!(
        argv.iter()
            .any(|part| part.to_ascii_lowercase().ends_with("npx-cli.js")),
        "argv must include npx-cli.js: {argv:?}"
    );
    assert_eq!(argv[argv.len() - 2], "-y");
    assert_eq!(
        argv[argv.len() - 1],
        "@agentclientprotocol/codex-acp@1.10.0"
    );
    let _ = fs::remove_dir_all(dir);
    let _ = fs::remove_dir_all(cache);
}

#[cfg(all(windows, feature = "server"))]
#[test]
fn npx_wrapper_real_launcher_shape_unwraps_to_node_and_npx_cli_js() {
    // Uses the real npm launcher shape (npx.cmd as shipped by npm 10.x),
    // NOT the per-package cmd-shim shape. The per-package shape test
    // above passed before the fix; this test with the REAL launcher
    // shape is the one that was red because the old code did not
    // recognize it.
    let dir = temporary_directory("npx-real-launcher");
    fs::create_dir_all(dir.join("node_modules").join("npm").join("bin")).expect("npm bin");
    File::create(dir.join("node.exe")).expect("node");
    fs::write(
            dir.join("npx.cmd"),
            "\
SET \"NODE_EXE=%~dp0\\node.exe\"\n\
IF NOT EXIST \"%NODE_EXE%\" ( SET \"NODE_EXE=node\" )\n\
SET \"NPM_PREFIX_JS=%~dp0\\node_modules\\npm\\bin\\npm-prefix.js\"\n\
SET \"NPX_CLI_JS=%~dp0\\node_modules\\npm\\bin\\npx-cli.js\"\n\
FOR /F \"delims=\" %%F IN ('CALL \"%NODE_EXE%\" \"%NPM_PREFIX_JS%\"') DO ( SET \"NPM_PREFIX_NPX_CLI_JS=%%F\\node_modules\\npm\\bin\\npx-cli.js\" )\n\
IF EXIST \"%NPM_PREFIX_NPX_CLI_JS%\" ( SET \"NPX_CLI_JS=%NPM_PREFIX_NPX_CLI_JS%\" )\n\
\"%NODE_EXE%\" \"%NPX_CLI_JS%\" %*\n",
        )
        .expect("npx.cmd");
    fs::write(
        dir.join("node_modules")
            .join("npm")
            .join("bin")
            .join("npx-cli.js"),
        "/* npx */\n",
    )
    .expect("npx-cli.js");
    let cache = temporary_directory("npx-real-launcher-cache");
    fs::create_dir_all(&cache).expect("cache");
    let catalog =
        super::discover_catalog_in_paths(&FixtureFetch, &cache, std::slice::from_ref(&dir));
    let codex = catalog
        .agents
        .iter()
        .find(|agent| agent.id == "codex-acp")
        .expect("codex-acp from registry");
    let argv = codex
        .acp_command
        .as_ref()
        .expect("npx-wrapper with real launcher shape must resolve an ACP command");
    assert!(
        !argv[0].to_ascii_lowercase().contains(".cmd"),
        "argv[0] must be node.exe, not a .cmd shim: {}",
        argv[0]
    );
    assert!(
        argv.iter()
            .any(|part| part.to_ascii_lowercase().ends_with("npx-cli.js")),
        "argv must include npx-cli.js: {argv:?}"
    );
    assert_eq!(argv[argv.len() - 2], "-y");
    assert_eq!(
        argv[argv.len() - 1],
        "@agentclientprotocol/codex-acp@1.10.0"
    );
    let _ = fs::remove_dir_all(dir);
    let _ = fs::remove_dir_all(cache);
}

#[cfg(all(windows, feature = "server"))]
#[test]
fn npx_wrapper_command_is_none_when_npx_cmd_is_not_a_shim() {
    let dir = temporary_directory("npx-nonsim");
    fs::create_dir_all(&dir).expect("temporary directory");
    File::create(dir.join("npx.cmd")).expect("non-shim npx.cmd");
    let cache = temporary_directory("npx-nonsim-cache");
    fs::create_dir_all(&cache).expect("cache");
    let catalog =
        super::discover_catalog_in_paths(&FixtureFetch, &cache, std::slice::from_ref(&dir));
    let codex = catalog
        .agents
        .iter()
        .find(|agent| agent.id == "codex-acp")
        .expect("codex-acp from registry");
    assert_eq!(
        codex.acp_command, None,
        "a leftover .cmd/.bat argv[0] is not an ACP agent: {:?}",
        codex.acp_command
    );
    let _ = fs::remove_dir_all(dir);
    let _ = fs::remove_dir_all(cache);
}

#[cfg(feature = "server")]
#[test]
fn npx_wrapper_command_is_none_when_npx_is_absent_from_path() {
    let dir = temporary_directory("npx-absent");
    fs::create_dir_all(&dir).expect("temporary directory");
    let cache = temporary_directory("npx-absent-cache");
    fs::create_dir_all(&cache).expect("cache");
    let catalog =
        super::discover_catalog_in_paths(&FixtureFetch, &cache, std::slice::from_ref(&dir));
    let codex = catalog
        .agents
        .iter()
        .find(|agent| agent.id == "codex-acp")
        .expect("codex-acp from registry");
    assert_eq!(
        codex.acp_command, None,
        "npx missing from PATH must not invent an ACP command: {:?}",
        codex.acp_command
    );
    let _ = fs::remove_dir_all(dir);
    let _ = fs::remove_dir_all(cache);
}

/// C-1's other half: the id a caller stores or compares comes from the same
/// match that fills `tools`, so the two cannot drift into admitting an id
/// the lookup does not serve, or the reverse.
#[test]
fn the_canonical_id_agrees_with_the_tool_lookup() {
    // S9: pi and codex are served rows now (carriers S5/S6, verified S7/S8).
    for spelling in [
        "claude", "CLAUDE", "Claude", "grok", "GROK", "Grok", "pi", "PI", "Pi", "codex", "CODEX",
        "Codex",
    ] {
        let canonical = super::mcp_catalog_id(spelling).expect("a catalog id");
        assert_eq!(
            super::mcp_catalog_id(canonical),
            Some(canonical),
            "the catalog's own id must resolve to itself"
        );
        assert!(!super::mcp_tools_for(spelling).is_empty());
        assert_eq!(
            super::mcp_tools_for(spelling),
            super::mcp_tools_for(canonical),
            "{spelling} and {canonical} are one provider and must be served one tool list"
        );
    }
    // A name the catalog does not publish is no id and is served no tools:
    // the predicate the policy store admits a row with, on both sides.
    for unknown in ["claude-acp", "does-not-exist", ""] {
        assert_eq!(super::mcp_catalog_id(unknown), None, "{unknown}");
        assert!(super::mcp_tools_for(unknown).is_empty(), "{unknown}");
    }
}

/// The property the audit attacks: no `(preset, provider)` cell resolves to
/// a mode a session nobody is watching could be run in.
///
/// Written as a walk over the table rather than a list of expected modes, so
/// a cell added later is checked even if nobody remembers this test exists.
#[test]
fn no_preset_cell_resolves_to_an_unattended_mode() {
    let mut checked = 0usize;
    for preset in super::AGENT_PRESETS {
        for cell in preset.cells {
            checked += 1;
            assert!(
                !super::mode_is_unattended(cell.provider, cell.mode),
                "preset {} provider {} resolves to unattended mode {}",
                preset.id,
                cell.provider,
                cell.mode
            );
        }
    }
    assert_eq!(
        checked,
        super::AGENT_PRESETS.len() * super::PRESET_WORKER_CELLS.len(),
        "every cell of every preset must be walked"
    );
    assert!(
        checked >= 6,
        "the table must cover the catalog it claims to"
    );
}

/// The named exclusions themselves, so a table that resolved to one is
/// caught even if the walk above were pointed at the wrong list.
#[test]
fn the_named_exclusions_are_unattended_and_the_allowed_modes_are_not() {
    for (provider, mode) in [
        ("pi", "bypass"),
        ("codex", "full-access"),
        ("codex", "auto-review"),
        ("claude", "bypassPermissions"),
        ("claude", "acceptEdits"),
        ("claude", "auto"),
        ("grok", "auto_accept"),
    ] {
        assert!(
            super::mode_is_unattended(provider, mode),
            "{provider} {mode} must be excluded"
        );
    }
    // The allowed cells, one by one: Codex's `auto` is the mode most likely
    // to be swept up by a careless list, and it is the allowed one.
    for (provider, mode) in [
        ("pi", "ask"),
        ("codex", "auto"),
        ("claude", "default"),
        ("grok", "default"),
    ] {
        assert!(
            !super::mode_is_unattended(provider, mode),
            "{provider} {mode} is an allowed mode"
        );
    }
}

/// Every catalog provider has a cell in every preset, and the `design`
/// overlay is the only one that removes tools.
#[test]
fn every_catalog_provider_has_a_cell_and_only_design_has_an_overlay() {
    let providers = super::catalog_provider_ids();
    for preset in super::AGENT_PRESETS {
        for provider in &providers {
            // Every provider the catalog publishes has a cell in every
            // preset, the debug-only rows included (audit S5B-08): those
            // are the rows the slice-5 battery drives, so a preset that
            // stopped naming one fails here rather than in the battery.
            assert!(
                super::resolve_agent_preset(preset.id, provider).is_ok(),
                "preset {} provider {provider}",
                preset.id
            );
        }
        for cell in preset.cells {
            let design = preset.id == super::AGENT_PRESET_DESIGN;
            assert_eq!(
                cell.overlay.denied().is_empty(),
                !design,
                "only the design preset may carry an overlay"
            );
        }
    }
}

/// The `design` overlay removes exactly the two tools, and both presets
/// keep the roster.
#[test]
fn the_design_overlay_hides_send_and_create_and_keeps_the_roster() {
    let design = super::ToolOverlay::DESIGN;
    assert!(!design.allows(super::MCP_CREATE_AGENT_TOOL));
    assert!(!design.allows(super::MCP_SEND_MESSAGE_TOOL));
    assert!(design.allows(super::MCP_ROSTER_TOOL));
    assert_eq!(
        design.denied(),
        vec![super::MCP_SEND_MESSAGE_TOOL, super::MCP_CREATE_AGENT_TOOL]
    );
    let worker = super::ToolOverlay::NONE;
    for tool in [
        super::MCP_CREATE_AGENT_TOOL,
        super::MCP_SEND_MESSAGE_TOOL,
        super::MCP_ROSTER_TOOL,
    ] {
        assert!(worker.allows(tool), "{tool}");
    }
    // No overlay anywhere in the table may name a tool the catalog does not
    // publish: a typo would disable nothing and read as if it had.
    let published: Vec<&str> = super::MCP_BROKER_TOOLS
        .iter()
        .map(|(name, _)| *name)
        .collect();
    for overlay in super::overlay_names() {
        for disabled in overlay.denied() {
            assert!(published.contains(&disabled), "{disabled} is not a tool");
        }
    }
}

/// A repeated deny name normalises at construction: the store does not
/// refuse duplicates, and a live overlay must compare equal to its
/// resumed twin, which the write canonicalises.
#[test]
fn a_repeated_deny_name_normalises_at_construction() {
    let once = super::ToolOverlay::from_profile_names(&[super::MCP_SEND_MESSAGE_TOOL.to_string()]);
    let twice = super::ToolOverlay::from_profile_names(&[
        super::MCP_SEND_MESSAGE_TOOL.to_string(),
        super::MCP_CREATE_AGENT_TOOL.to_string(),
        super::MCP_SEND_MESSAGE_TOOL.to_string(),
    ]);
    let reordered = super::ToolOverlay::from_profile_names(&[
        super::MCP_CREATE_AGENT_TOOL.to_string(),
        super::MCP_SEND_MESSAGE_TOOL.to_string(),
    ]);
    assert_eq!(twice, reordered);
    assert!(!twice.allows(super::MCP_SEND_MESSAGE_TOOL));
    assert!(!twice.allows(super::MCP_CREATE_AGENT_TOOL));
    assert!(twice.allows(super::MCP_ROSTER_TOOL));
    assert_ne!(twice, once);
}

/// The refusals are one sentence each, and an unknown preset does not read
/// as an unknown provider.
#[test]
#[cfg(debug_assertions)]
fn the_release_table_never_names_a_test_provider() {
    for preset in super::AGENT_PRESETS {
        for cell in preset.cells {
            assert!(
                !matches!(cell.provider, "devboule-acp-stub" | "devboule-absent-probe"),
                "{} names {} in the release table",
                preset.id,
                cell.provider
            );
        }
    }
    assert!(super::test_only_cell(super::AGENT_PRESET_WORKER, "devboule-acp-stub").is_some());
    assert!(super::test_only_cell(super::AGENT_PRESET_DESIGN, "devboule-absent-probe").is_some());
    assert!(super::test_only_cell("nowhere", "devboule-acp-stub").is_none());
}

#[test]
fn preset_resolution_refuses_unknown_presets_providers_and_bare_providers() {
    assert_eq!(
        super::resolve_agent_preset("manager", "claude").unwrap_err(),
        "unknown preset"
    );
    assert_eq!(
        super::resolve_agent_preset("worker", "does-not-exist").unwrap_err(),
        "unknown provider"
    );
    assert_eq!(
        super::resolve_agent_preset("worker", "claude-code")
            .unwrap()
            .1
            .mode,
        "default",
        "an alias resolves to the catalog's own row"
    );
}

/// The preamble is one sentence and it asks for nothing but the result.
#[test]
fn preambles_hold_no_instruction_to_write_files() {
    for preset in super::AGENT_PRESETS {
        let lowered = preset.preamble.to_lowercase();
        for word in [
            "write",
            "file",
            "files",
            "save",
            "path",
            "output to",
            ".md",
            "artifact",
        ] {
            assert!(
                !lowered.contains(word),
                "preset {} preamble mentions {word}: {}",
                preset.id,
                preset.preamble
            );
        }
        assert!(
            preset.preamble.contains("final message"),
            "preset {} must ask for the result in the final message",
            preset.id
        );
    }
}
/// The `unattended` derivation is keyed on **authorship**, never on a
/// provider name, and never on the profile's `autoAccept` tick (R2b): the
/// delivered mode is what the birth judges, route A is the daemon's own
/// broker list, route B is each client family's own dictionary, and a
/// vocabulary the daemon did not author answers `unknown`. The old test
/// this one replaces pinned the two-disjunct profile predicate the design
/// deleted; the coverage that survives it is re-expressed here against the
/// derivation that replaced it.
#[test]
fn the_unattended_derivation_is_keyed_on_authorship_and_never_on_the_tick() {
    use devboule_protocol::UnattendedState;
    let prediction = |provider: &str, mode: &str| {
        crate::peer_policy::unattended_mode(super::session_kind_for(provider), Some(mode))
    };
    // Route A: the three ids the daemon itself auto-answers a permission
    // request in, whatever the family — including a provider the catalog
    // has never heard of (the mechanical test: a user-defined provider's
    // child is answered without any code path noticing the name).
    for provider in [
        "grok",
        "codex",
        "claude",
        "a-provider-that-does-not-exist-yet",
    ] {
        for mode in ["bypass", "auto_accept", "bypassPermissions"] {
            assert_eq!(
                prediction(provider, mode),
                UnattendedState::Yes,
                "{provider} {mode}: the daemon's own broker answers it"
            );
        }
    }
    // The human's own toggle is **not an input**: the marker reads the
    // mode the delivery carries, and a tick can never move the answer.
    // (A tick over an asking mode is refused at creation by a different
    // check; the prediction here stays honest about what the mode does.)
    assert_eq!(
        prediction("a-provider-that-does-not-exist-yet", "ask"),
        UnattendedState::Unknown,
        "a provider-authored vocabulary cannot be established, tick or no tick"
    );
    // The kind resolution behind `prediction` is **case-sensitive** —
    // unlike the catalog row match, which is documented
    // case-insensitive — so a differently spelled name derives under ACP
    // and answers `unknown`. That asymmetry is deliberate and pinned:
    // the failure direction is an uncertainty, never a false certainty
    // (audit R2b-1 §5.1).
    assert_eq!(
        prediction("Claude", "default"),
        UnattendedState::Unknown,
        "`Claude` is not `claude`: the kind match is exact, and the miss \
             fails toward unknown"
    );
    // Route B: the daemon-authored knobs, in the client family's own
    // dictionary. Codex `full-access` is `approvalPolicy: never` in this
    // daemon's own turn parameters — the case that proves a route-A-only
    // boolean under-reports.
    assert_eq!(prediction("codex", "full-access"), UnattendedState::Yes);
    // A mode that still asks the human says `no`, from the same tables
    // that admit the mode at all.
    for (provider, mode) in [
        ("codex", "read-only"),
        ("codex", "auto"),
        ("claude", "acceptEdits"),
        ("claude", "auto"),
        ("claude", "default"),
        ("pi", "ask"),
    ] {
        assert_eq!(
            prediction(provider, mode),
            UnattendedState::No,
            "{provider} {mode} asks, and the daemon authored it"
        );
    }
    // Codex `auto-review` is the one authored id that answers `unknown`:
    // the peer gate counts it as a mode that can pass a permission moment
    // with nobody answering (`prompt_skipping_mode`), so `no` — which
    // renders as nothing — is the wrongly-benign badge; and it is not
    // `yes`, because a model reviewer may hand a moment back (audit
    // R2b-1 §3.3).
    assert_eq!(
        prediction("codex", "auto-review"),
        UnattendedState::Unknown,
        "the peer gate calls this id prompt-skipping, so the marker renders \
             the present unknown, never the benign nothing"
    );
    assert_eq!(prediction("pi", "bypass"), UnattendedState::Yes);
}

/// The one list the broker grants from and the birth marker reads: exactly
/// the three provider-agnostic ids, and no provider's own spelling.
#[test]
fn the_auto_answer_list_is_exactly_the_three_provider_agnostic_ids() {
    for mode in ["bypass", "auto_accept", "bypassPermissions"] {
        assert!(super::mode_is_auto_answered(mode), "{mode}");
    }
    for mode in [
        "ask",
        "default",
        "full-access",
        "auto-review",
        "acceptEdits",
        "auto",
        "",
    ] {
        assert!(!super::mode_is_auto_answered(mode), "{mode}");
    }
}

fn ticked() -> serde_json::Map<String, serde_json::Value> {
    serde_json::json!({ "autoAccept": true })
        .as_object()
        .expect("object")
        .to_owned()
}

/// The pre-card tick judgement (the re-audit's P1): a verdict that
/// refuses exists only where the daemon owns the rule — Claude and Pi —
/// and the daemon's own table always concludes *satisfied*, never
/// refused. Codex's `full-access` is the conviction: provider-authored
/// vocabulary, and the answer is `NotOursToJudge`, not a refusal.
#[test]
fn the_pre_card_tick_judgement_refuses_only_where_the_daemon_owns_the_rule() {
    let features = ticked();
    use super::AutoAcceptTick::*;
    // No tick: consistent for every family and every mode spelling.
    for provider in [
        "claude",
        "pi",
        "codex",
        "devboule-acp-stub",
        "someone-elses-agent",
    ] {
        for mode in ["default", "ask", "full-access", "bypass"] {
            assert_eq!(
                super::judge_auto_accept_tick(provider, mode, &serde_json::Map::new()),
                Consistent,
                "{provider} {mode}: no tick, no contradiction"
            );
        }
    }
    // A tick over a daemon-owned id: satisfied, whatever the family.
    for provider in ["claude", "pi", "codex", "devboule-acp-stub"] {
        for mode in super::auto_answered_modes() {
            assert_eq!(
                super::judge_auto_accept_tick(provider, mode, &features),
                Consistent,
                "{provider} {mode}: the broker answers this mode"
            );
        }
    }
    // The daemon-owned tick rules: Claude and Pi refuse any other mode id.
    for provider in ["claude", "pi"] {
        for mode in ["default", "ask", "acceptEdits", "plan"] {
            assert_eq!(
                super::judge_auto_accept_tick(provider, mode, &features),
                Contradicts,
                "{provider} {mode}: the tick demands a mode the broker answers"
            );
        }
    }
    // The families that own their knob: no pre-card conclusion at all —
    // including `full-access`, the case the old gate refused in error.
    for provider in [
        "codex",
        "devboule-acp-stub",
        "a-provider-from-a-config-file",
    ] {
        for mode in [
            "default",
            "ask",
            "full-access",
            "auto-review",
            "acceptEdits",
        ] {
            assert_eq!(
                super::judge_auto_accept_tick(provider, mode, &features),
                NotOursToJudge,
                "{provider} {mode}: not the daemon's fact to state"
            );
        }
    }
}

/// The repair pass's closed-dimension rule, applied to the pre-card
/// judgement: every `SessionKind` arm of `judge_auto_accept_tick` has a
/// pinned verdict, reached through each kind's own provider spelling,
/// so a family's verdict is a recorded decision rather than whatever a
/// wildcard happened to return (the re-audit's P2-5). The match in
/// `judge_auto_accept_tick` spells every arm with no wildcard, so
/// adding a sixth family is a compile error before this test can even
/// run — and this test then forces whoever adds it to write down what
/// the new family's verdict is.
#[test]
fn every_session_kind_has_a_pinned_pre_card_tick_verdict() {
    use super::AutoAcceptTick::*;
    use devboule_protocol::SessionKind;

    // The provider spellings each kind resolves from
    // (`session_kind_for`): the three authored families by name, every
    // other name — including a user-defined provider from a config
    // file — to the ACP family. `Terminal` is unreachable through
    // `session_kind_for` today, so its verdict is pinned through the
    // accessor itself, one arm at a time, below.
    let verdict_for_provider =
        |provider: &str| super::judge_auto_accept_tick(provider, "ask", &ticked());
    assert_eq!(verdict_for_provider("claude"), Contradicts);
    assert_eq!(verdict_for_provider("pi"), Contradicts);
    assert_eq!(verdict_for_provider("codex"), NotOursToJudge);
    assert_eq!(verdict_for_provider("devboule-acp-stub"), NotOursToJudge);
    assert_eq!(
        verdict_for_provider("someone-elses-agent-from-a-config-file"),
        NotOursToJudge
    );

    // The closed table, walked one arm at a time through the accessor
    // the match serves: adding a variant to `SessionKind` makes the
    // match in `judge_auto_accept_tick` fail to compile, and the new
    // arm must be given its verdict before this table can be extended.
    let kind_of_provider = |provider: &str| super::session_kind_for(provider);
    assert_eq!(kind_of_provider("claude"), SessionKind::Claude);
    assert_eq!(kind_of_provider("pi"), SessionKind::Pi);
    assert_eq!(kind_of_provider("codex"), SessionKind::Codex);
    assert_eq!(kind_of_provider("devboule-acp-stub"), SessionKind::Acp);
    assert_eq!(
        kind_of_provider("someone-elses-agent-from-a-config-file"),
        SessionKind::Acp
    );
}

/// The published schema of `devboule_create_agent` cannot express a provider,
/// a preset, a model, a mode or a feature: what an agent cannot say is what no
/// check of ours can get wrong (`S5` §2 rev 9).
#[test]
fn the_create_agent_schema_cannot_express_a_provider_or_a_preset() {
    let schema = super::agent_create_input_schema();
    let properties = schema["properties"].as_object().expect("properties");
    let mut names: Vec<&str> = properties.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "cwd",
            "initialPrompt",
            "labels",
            "notifyOnFinish",
            "profile",
            "title",
            "workspaceId",
        ]
    );
    let required: Vec<&str> = schema["required"]
        .as_array()
        .expect("required")
        .iter()
        .map(|name| name.as_str().expect("a name"))
        .collect();
    assert_eq!(required, ["profile", "title", "initialPrompt"]);
    for forbidden in [
        "provider", "preset", "model", "mode", "features", "settings",
    ] {
        assert!(
            !properties.contains_key(forbidden),
            "{forbidden} must not be expressible at all"
        );
    }
    assert_eq!(schema["additionalProperties"], serde_json::json!(false));
}
