//! Tests for the pi client: the rpc handshake, the tool perimeter and turn lifecycle.

use super::{
    bridge_extension, carried_pi_mime_types, is_bridge_notify, is_ready_notify, mcp_launch,
    perform_handshake, permission_extension_path, permission_request_from_ui, pi_control_frame,
    pi_delivery, pi_image_entry, pi_permission_sender, pi_prompt_frame, pi_steer_fields,
    plan_pi_prompt, spawn_args, thinking_level_allowed, write_permission_extension, PiCatalog,
    PiControl, PiReader, PiStaticPrompt, PiStdout, PiSteerer, PiSwitcher,
};
use crate::acp_view::PromptCapabilityState;
use crate::attachment_store::AttachmentStore;
use crate::pi_view::events_from_line;
use crate::raster_metadata::{clean_png, png_with_text_chunk, vector_input, vector_output};
use crate::session::{ModelSwitcher, PtyCommand, ReaderDispatch, SessionRuntime, StaticImageSink};
// The shared admission helper (A2-03): one place, so the Pi and the Codex
// steer tests exercise the same token `with_active_turn` hands out.
use crate::test_support::steer_through_the_turn;
use devboule_protocol::{PromptAttachment, SessionEvent};
use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;
use std::process::ChildStdin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// One `node` spawner for the Pi tests, so a missing binary fails the
/// same way everywhere.
fn node_command() -> std::process::Command {
    std::process::Command::new("node")
}

/// What a `node` spawn failure panics with: the test name, the program
/// name, the `io::Error` (kind + OS message), and the `PATH` and cwd the
/// test process saw — the context the old "node is required" message
/// never gave.
fn node_unavailable(test: &str, error: &std::io::Error) -> String {
    format!(
            "node is required for the {test}: could not spawn `node` ({error}; kind={:?}; PATH={:?}; cwd={:?})",
            error.kind(),
            std::env::var("PATH").unwrap_or_default(),
            std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        )
}

/// `get_available_models` as Pi answered it over `pi --mode rpc` (captured
/// 2026-09-10). The capture elided each model's `cost` object and the body
/// of `thinkingLevelMap`; those two omissions are the only difference from
/// the wire. Per-model `input` is the part under test, and it is verbatim.
const PI_MODELS_CAPTURE: &str = r#"{"data":{"models":[
{"id":"minimax-m3","name":"MiniMax-M3","api":"anthropic-messages","provider":"opencode-go","reasoning":true,"input":["text","image"],"contextWindow":1000000,"maxTokens":131072},
{"id":"deepseek-v4-flash","name":"DeepSeek V4 Flash","api":"openai-completions","reasoning":true,"input":["text"]},
{"id":"deepseek-v4-flash-vision-exp","name":"DeepSeek V4 Flash Vision Exp","reasoning":true,"input":["text","image"]}
]}}"#;

fn catalog_from_capture(capture: &str) -> PiCatalog {
    let state = serde_json::json!({
        "data": {
            "sessionId": "session-1",
            "model": {"id": "minimax-m3", "provider": "opencode-go"},
        }
    });
    let models: serde_json::Value = serde_json::from_str(capture).expect("captured models");
    let levels = serde_json::json!({"data": {"levels": ["high"]}});
    super::catalog_from_responses(&state, &models, &levels).expect("catalog")
}

#[test]
fn thinking_level_validation_is_against_the_current_model_list() {
    let levels = ["low".to_string(), "high".to_string(), "max".to_string()];
    assert!(!thinking_level_allowed("panzeroni", &levels));
    assert!(!thinking_level_allowed("xhigh", &levels));
    assert!(thinking_level_allowed("high", &levels));
}

#[test]
fn ask_mode_is_rejected_until_the_permission_extension_reports_in() {
    let active = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let switcher = PiSwitcher {
        control: Arc::new(PiControl::new(
            Arc::new(Mutex::new(None)),
            Arc::new(std::sync::atomic::AtomicU64::new(1)),
        )),
        catalog: Arc::new(Mutex::new(PiCatalog::default())),
        mode_id: Arc::new(Mutex::new("bypass".to_string())),
        permission_extension_active: Arc::clone(&active),
    };
    let error = switcher.set_mode("ask").expect_err("extension is inactive");
    assert_eq!(error.message, "Pi permission extension not active.");
    switcher.set_mode("bypass").expect("bypass is immediate");
    active.store(true, Ordering::Release);
    switcher.set_mode("ask").expect("extension is active");
}

#[test]
fn only_our_notify_is_the_permission_channel_ready_signal() {
    let ready = serde_json::json!({
        "type": "extension_ui_request",
        "method": "notify",
        "message": "devboule-permission-channel"
    });
    let session = serde_json::json!({"type": "session", "id": "session-1"});
    assert!(is_ready_notify(&ready));
    assert!(!is_ready_notify(&session));
}

#[test]
fn handshake_does_not_require_the_ready_signal() {
    // A2-13: the suite skips without `node` instead of turning a machine
    // that lacks it into a red build.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut child = node_command()
        .args(["-e", "setTimeout(() => {}, 10000)"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("{}", node_unavailable("Pi handshake test", &error)));
    let stdin = child.stdin.take().expect("node stdin");
    let stdin = std::sync::Mutex::new(Some(stdin));
    let mut stdout = PiStdout::spawn(std::io::Cursor::new(
            br#"{"id":"h-1","type":"response","success":true,"data":{"sessionId":"session-1","model":{"id":"m","provider":"p"}}}
{"id":"h-2","type":"response","success":true,"data":{"models":[{"id":"m","name":"M","provider":"p"}]}}
{"id":"h-3","type":"response","success":true,"data":{"levels":["medium"]}}
"#
            .to_vec(),
        ))
        .expect("stdout reader");
    let result = perform_handshake(
        &mut stdout,
        &stdin,
        &std::sync::atomic::AtomicU64::new(1),
        "bypass",
    );
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        result.is_ok(),
        "handshake must not wait for the extension notify: {:?}",
        result.err()
    );
}

#[test]
fn spawn_args_keep_rpc_and_add_our_extension() {
    let command = PtyCommand::new(
        "pi",
        Vec::new(),
        std::env::current_dir().expect("cwd"),
        Vec::new(),
    );
    let path = Path::new(r"C:\runtime\devboule-pi-permissions.ts");
    let args = spawn_args(&command, path, None).expect("Pi args");
    let mode = args.iter().position(|arg| arg == "--mode").expect("mode");
    let extension = args.iter().position(|arg| arg == "-e").expect("extension");
    assert!(mode < extension);
    assert_eq!(args[mode + 1], "rpc");
    assert_eq!(args[extension + 1], path.to_string_lossy());
    assert!(!args.iter().any(|arg| arg == "--no-extensions"));
    assert!(!args.iter().any(|arg| arg == "--tools"));
}

#[test]
fn caller_extensions_are_preserved_alongside_ours() {
    let command = PtyCommand::new(
        "pi",
        vec![
            "--extension".to_string(),
            "first.ts".to_string(),
            "--extension=second.ts".to_string(),
        ],
        std::env::current_dir().expect("cwd"),
        Vec::new(),
    );
    let path = Path::new("permission.ts");
    let args = spawn_args(&command, path, None).expect("caller extensions are valid");
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--extension".to_string(), "first.ts".to_string()]));
    assert!(args.iter().any(|arg| arg == "--extension=second.ts"));
    assert_eq!(args.iter().filter(|arg| *arg == "-e").count(), 1);
    let path = path.to_string_lossy().into_owned();
    assert!(args.iter().any(|arg| arg == &path));
}

#[test]
fn only_non_rpc_pi_modes_are_rejected() {
    for supplied in [
        vec!["--mode"],
        vec!["--mode", "interactive"],
        vec!["--mode=interactive"],
    ] {
        let command = PtyCommand::new(
            "pi",
            supplied.iter().map(|arg| (*arg).to_string()).collect(),
            std::env::current_dir().expect("cwd"),
            Vec::new(),
        );
        spawn_args(&command, Path::new("permission.ts"), None)
            .expect_err("non-rpc mode must be rejected");
    }
    for supplied in [
        vec!["--mode", "rpc"],
        vec!["--mode=rpc"],
        vec!["--extension", "evil.ts"],
        vec!["-e=evil.ts"],
    ] {
        let command = PtyCommand::new(
            "pi",
            supplied.iter().map(|arg| (*arg).to_string()).collect(),
            std::env::current_dir().expect("cwd"),
            Vec::new(),
        );
        spawn_args(&command, Path::new("permission.ts"), None)
            .expect("rpc mode and caller extensions are valid");
    }
}

#[test]
fn wire_shaped_pi_confirm_keeps_permission_command_metadata() {
    let request = permission_request_from_ui(
        &serde_json::json!({
            "type": "extension_ui_request",
            "method": "confirm",
            "id": "ui-1",
            "title": {
                "title": "Devboule permission",
                "message": "Allow bash?",
                "command": "bash",
                "args": ["command=pi", "-p", "touch outside.txt"],
                "cwd": "C:/workspace"
            }
        }),
        "ui-1",
    );
    assert!(matches!(
        request,
        SessionEvent::PermissionRequest {
            command: Some(command),
            args: Some(args),
            cwd: Some(cwd),
            ..
        } if command == "bash"
            && args
                == vec![
                    "command=pi".to_string(),
                    "-p".to_string(),
                    "touch outside.txt".to_string(),
                ]
            && cwd == "C:/workspace"
    ));
}

#[test]
fn pi_permission_sender_keeps_control_after_failed_write() {
    let stdin: Arc<Mutex<Option<std::process::ChildStdin>>> = Arc::new(Mutex::new(None));
    let controls = Arc::new(Mutex::new(HashMap::from([(7, "ui-7".to_string())])));
    let sender = pi_permission_sender(Arc::clone(&stdin), Arc::clone(&controls));

    assert!((sender)(7, serde_json::json!({"outcome": {"outcome": "cancelled"}})).is_err());
    assert!(controls.lock().expect("controls").contains_key(&7));
}

#[test]
fn pi_auto_answer_failure_still_denies_the_extension_confirm() {
    // A2-13: a machine without `node` skips this rather than failing it.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut child = node_command()
        .args([
            "-e",
            "process.stdin.on('data', data => process.stdout.write(data))",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("{}", node_unavailable("Pi auto-answer test", &error)));
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let mut stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    let broker = super::PermissionBroker::for_test(Arc::new(|_, _| {
        Err(std::io::Error::other("synthetic completion failure"))
    }));
    let mut reader = super::PiReader::new(
        Vec::new(),
        SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "bypass".to_string(),
                available_modes: Vec::new(),
            }),
        },
        Arc::clone(&broker),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(std::sync::atomic::AtomicU64::new(1)),
        Arc::new(super::PiControl::new(
            Arc::clone(&stdin),
            Arc::new(std::sync::atomic::AtomicU64::new(1)),
        )),
        Arc::clone(&stdin),
        Arc::new(std::sync::atomic::AtomicBool::new(true)),
    );
    let runtime = Arc::new(crate::session::SessionRuntime::new());
    let request = serde_json::json!({
        "type": "extension_ui_request",
        "method": "confirm",
        "id": "ui-1",
        "title": {"title": "Pi permission", "message": "Allow bash?"},
        "command": "bash"
    });
    reader
        .feed(format!("{request}\n").as_bytes(), &runtime)
        .expect("confirm request");
    let mut line = String::new();
    stdout
        .read_line(&mut line)
        .expect("definite extension response");
    let response: serde_json::Value = serde_json::from_str(&line).expect("response json");
    assert_eq!(response["type"], "extension_ui_response");
    assert_eq!(response["id"], "ui-1");
    assert_eq!(response["confirmed"], false);
    assert_eq!(broker.pending_len(), 0);
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn permission_extension_paths_are_unique_per_session() {
    let runtime = Path::new(r"C:\runtime");
    assert_ne!(
        permission_extension_path(runtime),
        permission_extension_path(runtime)
    );
}

#[test]
fn permission_extension_prompts_unknown_tools_and_allows_confirmed_tools() {
    // A2-13: a machine without `node` skips this rather than failing it.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let path = crate::test_dirs::test_temp_dir("devboule-pi-permission-test").join("extension.mjs");
    write_permission_extension(&path).expect("permission extension");
    let script = r#"
(async () => {
  const { pathToFileURL } = require("url");
  const extension = await import(pathToFileURL(process.argv[1]).href);
  const handlers = {};
  extension.default({ on(name, handler) { handlers[name] = handler; } });
  const confirmations = [];
  const context = {
    cwd: "C:/workspace",
    ui: {
      notify() {},
      async confirm(request) {
        const confirmed = request.command !== "tool-futuro";
        confirmations.push({ request, confirmed });
        return confirmed;
      },
    },
  };
  for (const toolName of ["read", "grep", "find", "ls"]) {
    const result = await handlers.tool_call({ toolName, input: {} }, context);
    if (result !== undefined) process.exit(4);
  }
  const allowed = await handlers.tool_call({
    toolName: "write",
    input: { command: "pi -p touch outside.txt" },
  }, context);
  const allowedBash = await handlers.tool_call({
    toolName: "bash",
    input: { command: "pi -p touch outside.txt" },
  }, context);
  const denied = await handlers.tool_call({
    toolName: "tool-futuro",
    input: {},
  }, context);
  if (allowed !== undefined || allowedBash !== undefined || denied?.block !== true) process.exit(1);
  if (confirmations.length !== 3
      || confirmations[0].request.command !== "write"
      || confirmations[0].confirmed !== true
      || confirmations[1].request.command !== "bash"
      || confirmations[1].confirmed !== true
      || confirmations[0].request.args[0] !== "command=pi -p touch outside.txt"
      || confirmations[2].request.command !== "tool-futuro"
      || confirmations[2].confirmed !== false) {
    process.exit(2);
  }
})().catch((error) => { console.error(error); process.exit(3); });
"#;
    let output = node_command()
        .args(["-e", script, &path.to_string_lossy()])
        .output()
        .unwrap_or_else(|error| panic!("{}", node_unavailable("Pi gate behavior test", &error)));
    let _ = std::fs::remove_file(&path);
    assert!(
        output.status.success(),
        "Pi permission extension behavior failed (exit={}): {}{}",
        output
            .status
            .code()
            .map_or_else(|| "no exit code".to_string(), |code| code.to_string()),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn pi_broker_tools_are_unmediated_and_walked() {
    // S3 walking test: every tool the broker serves is classified exactly
    // once by the same constants the extension renders — unmediated with a
    // reason (all eleven today), never silently inheriting either answer. An
    // eighth broker tool with no row here fails the first assertion; a
    // `devboule_*` name missing from the unmediated set fails the second.
    for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
        assert!(
            super::is_unmediated_tool(name),
            "broker tool {name} must be unmediated (daemon-decided)"
        );
    }
    // The rendered set states the fact: the new identifier is present and
    // the old read-only name survives nowhere.
    let rendered = super::permission_extension();
    assert!(
        rendered.contains("unmediated"),
        "the extension renders the unmediated set"
    );
    assert!(
        !rendered.contains("__UNMEDIATED_TOOLS__"),
        "the placeholder is substituted, not left verbatim"
    );
    assert!(
        !rendered.contains("readOnly") && !rendered.contains("__READ_ONLY_TOOLS__"),
        "the false read-only name survives nowhere"
    );
    // The gate itself is unchanged: writes still ask, reads still pass.
    assert!(!super::is_unmediated_tool("write"));
    assert!(!super::is_unmediated_tool("bash"));
    assert!(!super::is_unmediated_tool("tool-futuro"));
    for name in ["read", "grep", "find", "ls"] {
        assert!(super::is_unmediated_tool(name), "{name} stays unmediated");
    }
    // Every rendered name is a known one: nothing unmediated by accident.
    let start = rendered.find('[').expect("rendered tool list");
    let end = rendered[start..].find(']').expect("rendered tool list end") + start;
    let list: Vec<String> =
        serde_json::from_str(&rendered[start..=end]).expect("rendered list is JSON");
    for name in &list {
        let known_pi = super::PI_TOOL_POLICIES.iter().any(|tool| tool.name == name);
        assert!(known_pi, "rendered {name} comes from the policy table");
    }
    for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
        assert!(
            list.iter().any(|rendered| rendered == name),
            "broker tool {name} reaches the rendered extension"
        );
    }
}

#[test]
fn pi_bridge_template_serves_the_broker_tools() {
    // S5 walking test for the bridge: every served tool's name and verbatim
    // description reaches the exact string `write_bridge_extension` persists —
    // an eighth tool, or a catalog rewording without a bridge edit, fails.
    // Hygiene markers ride the same test: dual Accept, named timeout, bridge
    // announce, env-identity Bearer, result/error + 202 handling.
    let template = bridge_extension();
    assert!(template.contains("import { Type }"), "typebox builders");
    for (name, description) in crate::provider_catalog::MCP_BROKER_TOOLS {
        assert!(
            template.contains(name),
            "bridge registers broker tool {name}"
        );
        assert!(
            template.contains(description),
            "bridge carries the catalog description for {name}"
        );
    }
    assert!(
        template.contains("Accept\": \"application/json, text/event-stream\"")
            || template.contains("Accept: \"application/json, text/event-stream\""),
        "dual Accept takes the broker JSON branch"
    );
    assert!(
        template.contains("MCP_TIMEOUT_MS") && template.contains("AbortSignal.timeout"),
        "every MCP fetch races the named timeout (spike S3b hung 80 s without one)"
    );
    assert!(
        template.contains("devboule-mcp-bridge"),
        "bridge announce rides session_start like the permission channel"
    );
    assert!(
        template.contains("mcpRequest(\"tools/list\", {}, undefined)"),
        "session_start proves in-band with an authenticated tools/list (S8 producer)"
    );
    assert!(
        template.contains("process.env.DEVBOULE_MCP_URL")
            && template.contains("process.env.DEVBOULE_MCP_TOKEN"),
        "identity by environment"
    );
    assert!(
        template.contains("Authorization: `Bearer ${MCP_TOKEN}`")
            || template.contains("Authorization\": `Bearer ${MCP_TOKEN}`"),
        "Bearer flies on every request (spike S5)"
    );
    assert!(
        template.contains("payload.error") || template.contains("payload && payload.error"),
        "RPC errors ride HTTP 200: parse result/error, never the status"
    );
    assert!(
        template.contains("202"),
        "202-empty is success without a result (never parsed, never failed)"
    );
    assert!(
        template.contains("devboule broker unreachable at"),
        "failures name the broker URL + cause, never bare fetch failed"
    );
    assert!(
        !template.contains("SPIKE_DUMP") && !template.contains("getSystemPrompt"),
        "no spike instrumentation ships"
    );
    assert!(
        !template.contains("process.argv"),
        "argv never carries identity"
    );
}

#[test]
fn pi_bridge_announce_is_detected_not_discovered() {
    // No rpc tool enumeration exists (spike-measured): the notify is the only
    // out-of-band readiness signal. S8 consumes this plus the in-child round-trip.
    let bridge = serde_json::json!({
        "type": "extension_ui_request",
        "method": "notify",
        "message": "devboule-mcp-bridge",
    });
    assert!(is_bridge_notify(&bridge));
    let permission = serde_json::json!({
        "type": "extension_ui_request",
        "method": "notify",
        "message": "devboule-permission-channel",
    });
    assert!(!is_bridge_notify(&permission));
    assert!(is_ready_notify(&permission));
    assert!(!is_ready_notify(&bridge));
    assert!(!is_bridge_notify(&serde_json::json!({"type": "session"})));
}

#[test]
fn pi_spawn_args_put_the_bridge_second_and_keep_callers() {
    // S5 argv shape: permission first, bridge second (the spike's measured
    // order), everything before `--`, caller forms preserved, token-free.
    let command = PtyCommand::new(
        "pi",
        vec!["--extension".to_string(), "first.ts".to_string()],
        std::env::current_dir().expect("cwd"),
        Vec::new(),
    );
    let permission = Path::new(r"C:\runtime\devboule-pi-permissions-1.ts");
    let bridge = Path::new(r"C:\runtime\devboule-pi-bridge-2.ts");
    let args = spawn_args(&command, permission, Some(bridge)).expect("bridge args");
    let dashes: Vec<usize> = args
        .iter()
        .enumerate()
        .filter(|(_, arg)| *arg == "-e")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(dashes.len(), 2, "permission -e plus bridge -e: {args:?}");
    assert_eq!(args[dashes[0] + 1], permission.to_string_lossy());
    assert_eq!(args[dashes[1] + 1], bridge.to_string_lossy());
    assert!(dashes[0] < dashes[1], "permission first, bridge second");
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--extension".to_string(), "first.ts".to_string()]));
    // Token-free argv: the bearer travels as env, never here.
    for arg in &args {
        assert!(!arg.contains("secret-bearer"), "no secret in argv: {arg}");
    }
    // And `None` stays the S3 shape: exactly one -e.
    let plain = spawn_args(&command, permission, None).expect("plain args");
    assert_eq!(plain.iter().filter(|arg| *arg == "-e").count(), 1);
}

#[test]
fn pi_mcp_launch_separates_env_from_argv() {
    // S4 seam body: env carries URL + token values, argv carries nothing,
    // the bridge file exists with the served names, owned_paths names it.
    let dir = crate::test_dirs::test_temp_dir("devboule-pi-mcp-launch");
    let config = crate::mcp_broker::McpLaunchConfig::for_test(
        "http://127.0.0.1:4321/mcp",
        "secret-bearer-launch",
    );
    let carrier = mcp_launch(&config, &dir).expect("carrier");
    assert!(carrier.arg_additions.is_empty(), "no verbatim argv splice");
    assert!(carrier.owned_dirs.is_empty(), "pi owns no dirs");
    let env: std::collections::HashMap<_, _> = carrier.env_additions.iter().cloned().collect();
    assert_eq!(
        env.get(crate::mcp_broker::MCP_URL_ENV).map(String::as_str),
        Some("http://127.0.0.1:4321/mcp")
    );
    assert_eq!(
        env.get(crate::mcp_broker::MCP_TOKEN_ENV)
            .map(String::as_str),
        Some("secret-bearer-launch")
    );
    assert_eq!(carrier.owned_paths.len(), 1);
    let bridge = std::fs::read_to_string(&carrier.owned_paths[0]).expect("bridge file");
    for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
        assert!(bridge.contains(name), "persisted bridge serves {name}");
    }
    assert!(
        !bridge.contains("secret-bearer-launch"),
        "no secret on disk"
    );
    let name = carrier.owned_paths[0]
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    assert!(
        name.starts_with("devboule-pi-bridge-") && name.ends_with(".ts"),
        "owned bridge name the S4 sweep covers: {name}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pi_bridge_drives_the_broker_shape_through_a_stub_pi() {
    // S5 executable analogue of spike S2/S3/S5 (which measured against the
    // real pi binary + stub broker/model): the persisted bridge file loads in
    // node against a stubbed `pi` object and a stubbed `fetch`, proving the
    // registerTool wiring, the Bearer + dual-Accept + string-body shape, the
    // URL-naming failure text, the RPC-error-on-200 parse, and the closed
    // schemas — without a pi binary or a socket.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let dir = crate::test_dirs::test_temp_dir("devboule-pi-bridge-test");
    std::fs::write(dir.join("bridge.mjs"), bridge_extension()).expect("bridge file");
    // A minimal `typebox` stub: the bridge only needs the builders to record
    // their arguments; the assertions below read the recorded schemas back.
    // (The real pi resolves its bundled typebox; spelling is identical.)
    let stub_dir = dir.join("node_modules").join("typebox");
    let _ = std::fs::create_dir_all(&stub_dir);
    std::fs::write(
        stub_dir.join("package.json"),
        r#"{"name":"typebox","version":"0.0.0-test","type":"module","main":"index.js"}"#,
    )
    .expect("stub package");
    std::fs::write(
        stub_dir.join("index.js"),
        r#"
export const Type = {
  Object: (properties, opts = {}) => ({ type: "object", properties, ...opts }),
  String: (opts = {}) => ({ type: "string", ...opts }),
  Boolean: (opts = {}) => ({ type: "boolean", ...opts }),
  Optional: (inner) => ({ ...inner, optional: true }),
  Integer: (opts = {}) => ({ type: "integer", ...opts }),
  Record: (k, v, opts = {}) => ({ type: "object", record: true, ...opts }),
  Union: (anyOf) => ({ anyOf }),
  Literal: (value) => ({ const: value }),
};
"#,
    )
    .expect("stub index");
    let script = r#"
(async () => {
  const path = require("path");
  const { pathToFileURL } = require("url");
  const tmp = process.argv[1];
  const calls = [];
  let behavior = "ok";
  global.fetch = async (url, opts) => {
    calls.push({ url, headers: opts.headers, body: opts.body });
    if (behavior === "refused") {
      const error = new Error("fetch failed");
      error.cause = { code: "ECONNREFUSED" };
      throw error;
    }
    if (behavior === "rpc-error") {
      return { ok: true, status: 200, text: async () => JSON.stringify({ jsonrpc: "2.0", id: 1, error: { code: -32601, message: "Unknown tool" } }) };
    }
    const req = JSON.parse(opts.body);
    if (req.method === "notifications/initialized") {
      return { ok: true, status: 202, text: async () => "" };
    }
    if (req.method === "initialize") {
      return { ok: true, status: 200, text: async () => JSON.stringify({ jsonrpc: "2.0", id: req.id, result: { protocolVersion: "2025-06-18", capabilities: {}, serverInfo: { name: "devboule", version: "1" } } }) };
    }
    return { ok: true, status: 200, text: async () => JSON.stringify({ jsonrpc: "2.0", id: req.id, result: { content: [{ type: "text", text: "STUB-OK" }], structuredContent: {} } }) };
  };
  const tools = {};
  const handlers = {};
  const notifies = [];
  const pi = {
    on(name, handler) { handlers[name] = handler; },
    registerTool(definition) { tools[definition.name] = definition; },
  };
  const extension = await import(pathToFileURL(path.join(tmp, "bridge.mjs")).href);
  extension.default(pi);
  await handlers.session_start({}, { ui: { notify: (message, kind) => notifies.push([message, kind]) } });
  if (!notifies.some(([message]) => message === "devboule-mcp-bridge")) process.exit(11);
  for (const name of ["devboule_list_agents", "devboule_list_profiles", "devboule_send_message", "devboule_create_agent", "devboule_set_agent_profile", "devboule_answer_permission"]) {
    if (!tools[name]) { console.error("missing tool " + name); process.exit(12); }
  }
  // S8 producer: session_start proves in-band with an authenticated tools/list.
  const listCalls = calls.filter((call) => {
    try { return JSON.parse(call.body).method === "tools/list"; } catch { return false; }
  });
  if (listCalls.length < 1) process.exit(25);
  if (listCalls[0].headers.Authorization !== `Bearer ${process.env.DEVBOULE_MCP_TOKEN}`) process.exit(26);
  const result = await tools.devboule_list_agents.execute("t1", {}, undefined);
  const last = calls[calls.length - 1];
  if (last.headers.Authorization !== `Bearer ${process.env.DEVBOULE_MCP_TOKEN}`) process.exit(13);
  if (last.headers.Accept !== "application/json, text/event-stream") process.exit(14);
  if (typeof last.body !== "string") process.exit(15);
  if (last.url !== process.env.DEVBOULE_MCP_URL) process.exit(16);
  if (!JSON.stringify(result).includes("STUB-OK")) process.exit(17);
  behavior = "refused";
  try {
    await tools.devboule_list_agents.execute("t2", {}, undefined);
    process.exit(18);
  } catch (error) {
    const text = String(error && error.message ? error.message : error);
    if (!text.includes(process.env.DEVBOULE_MCP_URL) || text === "fetch failed") process.exit(19);
  }
  behavior = "rpc-error";
  try {
    await tools.devboule_list_agents.execute("t3", {}, undefined);
    process.exit(20);
  } catch (error) {
    if (!String(error && error.message ? error.message : error).includes("Unknown tool")) process.exit(21);
  }
  const create = tools.devboule_create_agent.parameters;
  if (create.additionalProperties !== false) process.exit(22);
  for (const key of ["profile", "title", "initialPrompt"]) {
    if (!(create.required || []).includes(key)) process.exit(23);
  }
  const outcome = (((tools.devboule_answer_permission.parameters || {}).properties || {}).outcome || {});
  const values = outcome.anyOf ? outcome.anyOf.map((entry) => entry.const) : outcome.enum;
  if (!values || !values.includes("allow_once") || !values.includes("deny")) process.exit(24);
  // S8 tolerance: a failed startup list breaks nothing — the announce fired and
  // later calls still work. Refused broker, second startup, must resolve.
  behavior = "refused";
  const notified = notifies.length;
  await handlers.session_start({}, { ui: { notify: (message, kind) => notifies.push([message, kind]) } });
  if (notifies.length !== notified + 1) process.exit(27);
})().catch((error) => { console.error(error); process.exit(3); });
"#;
    let output = std::process::Command::new("node")
        .args(["-e", script, &dir.to_string_lossy()])
        .env("DEVBOULE_MCP_URL", "http://127.0.0.1:4321/mcp")
        .env("DEVBOULE_MCP_TOKEN", "stub-token-bridge")
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|error| panic!("{}", node_unavailable("Pi bridge behavior test", &error)));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "pi bridge behavior failed (exit={}): {}{}",
        output
            .status
            .code()
            .map_or_else(|| "no exit code".to_string(), |code| code.to_string()),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Attached-runtime helper mirroring ACP's (and Codex's): a runtime with a
/// broker plus a subscription whose published events the test can pull.
fn attached_runtime(
    session_id: &str,
    broker: Arc<super::super::permission_broker::PermissionBroker>,
) -> (
    Arc<SessionRuntime>,
    Arc<super::super::event_pull::ConnHandle>,
) {
    let runtime = SessionRuntime::for_acp(session_id.to_string(), None, Arc::clone(&broker));
    let conn = super::super::event_pull::ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    (runtime, conn)
}

/// One pi row derives two events — the finish and its context reading — and
/// BOTH must carry the row's journal seq. The attach seam drops a queued copy
/// only by seq (`remove_replayed_agent_items`), so a `None`-seq sibling
/// survives beside the copy replay derives from the same row and the reading
/// arrives twice.
#[test]
fn a_turn_end_line_delivers_each_event_exactly_once_across_a_fresh_attach() {
    use crate::journal::{new_session_record, Journal};

    let session_id = "s.pi.attach.once";
    let dir = crate::test_dirs::test_temp_dir("devboule-pi-attach-once");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            devboule_protocol::SessionKind::Pi,
            "Agent",
        ))
        .expect("upsert");
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
    }

    // No observer yet — the app is on another tab. The client journals the
    // frame and publishes its views into the shared backlog, exactly as a
    // live turn does.
    let stdin: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(None));
    let broker =
        super::super::permission_broker::PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = PiReader::new(
        Vec::new(),
        SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: None,
        },
        Arc::clone(&broker),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(AtomicU64::new(1)),
        Arc::new(PiControl::new(
            Arc::clone(&stdin),
            Arc::new(AtomicU64::new(1)),
        )),
        Arc::clone(&stdin),
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    // The recorded turn_end of `pi_view.rs`'s own fixture: totalTokens 25 851.
    let turn_end = serde_json::from_str::<serde_json::Value>(
        r#"{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"OK"}],"api":"openai-completions","provider":"openrouter","model":"z-ai/glm-5.3-flash","usage":{"input":25848,"output":3,"cacheRead":0,"cacheWrite":0,"reasoning":0,"totalTokens":25851,"cost":{"input":0.0019386,"output":7.5e-7,"cacheRead":0,"cacheWrite":0,"total":0.00193935}},"stopReason":"stop","timestamp":1788993862485,"responseId":"gen-1788993862-4cxcarrKksRnXEICsFHO","rawStopReason":"stop"},"toolResults":[]}"#,
    )
    .expect("recorded turn_end frame");
    reader
        .dispatch_value(turn_end, &runtime)
        .expect("dispatch the turn end");
    journal.flush().expect("flush");

    // The fresh attach replays the journaled row (both views) and scans the
    // backlog for copies the seam has to drop by seq.
    let conn = crate::session::ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let (mut finishes, mut readings) = (0_u64, 0_u64);
    loop {
        let batch = conn.pull_events();
        if batch.is_empty() {
            break;
        }
        for pending in &batch {
            match &pending.envelope.event {
                SessionEvent::AgentFinished { .. } => finishes += 1,
                SessionEvent::ContextUsage { .. } => readings += 1,
                _ => {}
            }
        }
    }
    assert_eq!(
        (finishes, readings),
        (1, 1),
        "one turn-end row delivers one finish and one reading; a None-seq \
         sibling would survive the seam beside its replayed twin"
    );

    drop(runtime);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pi_bearer_is_redacted_from_stderr_before_delivery() {
    // Broker-4, pi half: same defect as Codex (a bearer in the child env
    // since S9), same fix, same proof. No belt here — the
    // invalid-configuration marker is Codex's sentence, not pi's.
    let broker =
        super::super::permission_broker::PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let (runtime, conn) = attached_runtime("stderr-redaction-pi", broker);
    runtime.set_mcp_bearer("opaque-bearer-pi".to_string());
    runtime.set_mcp_url("http://127.0.0.1:4567/mcp".to_string());
    super::publish_stderr_line(
        &runtime,
        "pi echoed Bearer opaque-bearer-pi at http://127.0.0.1:4567/mcp".to_string(),
    );
    let event = conn
        .pull_events()
        .into_iter()
        .find_map(|event| match event.envelope.event {
            SessionEvent::AgentStderr { data } => Some(data),
            _ => None,
        })
        .expect("stderr event");
    assert_eq!(event, "pi echoed Bearer [redacted] at [redacted]");
    // And a clean line passes through verbatim (redaction, not blanking).
    super::publish_stderr_line(&runtime, "pi did something ordinary".to_string());
    let clean = conn
        .pull_events()
        .into_iter()
        .find_map(|event| match event.envelope.event {
            SessionEvent::AgentStderr { data } => Some(data),
            _ => None,
        })
        .expect("second stderr event");
    assert_eq!(clean, "pi did something ordinary");
}

#[test]
fn write_permission_extension_requires_the_runtime_parent() {
    let base = crate::test_dirs::test_temp_dir("devboule-pi-missing-parent");
    let path = base.join("absent").join("permission.ts");
    let error = write_permission_extension(&path).expect_err("missing parent must fail");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn exact_recorded_text_and_turn_lines_translate_without_message_end_text() {
    let text = serde_json::from_str::<serde_json::Value>(r#"{"type":"message_update","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"OK"}}"#).expect("recording");
    let text_end = serde_json::from_str::<serde_json::Value>(r#"{"type":"message_update","usage":{"input":25848,"output":3,"cacheRead":0,"cacheWrite":0,"reasoning":0,"totalTokens":25851,"cost":{"input":0.0019386,"output":7.5e-7,"cacheRead":0,"cacheWrite":0,"total":0.00193935}},"assistantMessageEvent":{"type":"text_end","contentIndex":0,"content":"OK"}}"#).expect("recording");
    assert!(
        matches!(events_from_line(&text).as_slice(), [SessionEvent::AgentMessage { text, .. }] if text == "OK")
    );
    assert!(events_from_line(&text_end).is_empty());
}

#[test]
fn captured_models_keep_image_support_apart_per_model() {
    // The real reply: two models declare images and one does not, so the
    // answer is a property of the model, not of the session.
    let catalog = catalog_from_capture(PI_MODELS_CAPTURE);

    let minimax = catalog.input_kinds("minimax-m3").expect("minimax-m3");
    assert_eq!(minimax.image, PromptCapabilityState::Supported);
    assert_eq!(minimax.declared, vec!["text", "image"]);

    let flash = catalog
        .input_kinds("deepseek-v4-flash")
        .expect("deepseek-v4-flash");
    assert_eq!(flash.image, PromptCapabilityState::Unsupported);
    assert_eq!(flash.declared, vec!["text"]);

    let vision = catalog
        .input_kinds("deepseek-v4-flash-vision-exp")
        .expect("deepseek-v4-flash-vision-exp");
    assert_eq!(vision.image, PromptCapabilityState::Supported);
}

#[test]
fn input_array_keeps_kinds_we_do_not_model() {
    // A future kind must be kept, not dropped and not treated as an error.
    let capture = r#"{"data":{"models":[
{"id":"a","name":"A","provider":"p","input":["text","image","audio","video","hologram"]},
{"id":"b","name":"B","provider":"p","input":["text","audio"]}
]}}"#;
    let catalog = catalog_from_capture(capture);

    let a = catalog.input_kinds("a").expect("model a");
    assert_eq!(
        a.declared,
        vec!["text", "image", "audio", "video", "hologram"],
        "unknown kinds must survive verbatim"
    );
    assert_eq!(a.image, PromptCapabilityState::Supported);

    let b = catalog.input_kinds("b").expect("model b");
    assert_eq!(b.declared, vec!["text", "audio"]);
    assert_eq!(b.image, PromptCapabilityState::Unsupported);
}

#[test]
fn absent_or_malformed_input_is_not_a_refusal() {
    let capture = r#"{"data":{"models":[
{"id":"silent","name":"Silent","provider":"p"},
{"id":"malformed","name":"Malformed","provider":"p","input":"text"},
{"id":"empty","name":"Empty","provider":"p","input":[]}
]}}"#;
    let catalog = catalog_from_capture(capture);

    for model_id in ["silent", "malformed"] {
        let kinds = catalog.input_kinds(model_id).expect(model_id);
        assert_eq!(
            kinds.image,
            PromptCapabilityState::Absent,
            "{model_id} never declared anything; silence is not a refusal"
        );
        assert!(kinds.declared.is_empty());
    }

    // A present array is a declaration even when it is empty: it lists no
    // image, and that is an answer rather than silence.
    let empty = catalog.input_kinds("empty").expect("empty");
    assert_eq!(empty.image, PromptCapabilityState::Unsupported);
    assert!(empty.declared.is_empty());
}

// --- image delivery (the static route) --------------------------------
//
// The routing decision lives in `plan_pi_prompt`, tested here against
// the attachment store directly, without spawning a child — the same
// arrangement the ACP sibling seam's tests use. The wire shape of one
// entry is pinned against Paseo's measured `convertPromptInput` output
// (`{"type":"image","data":...,"mimeType":"image/png"}`), and the
// frame omission against `...(images?.length ? { images } : {})`.

struct PlanTempDir(std::path::PathBuf);

impl PlanTempDir {
    fn new(tag: &str) -> Self {
        let dir = crate::test_dirs::test_temp_dir(&format!("devboule-pi-plan-{tag}"));
        Self(dir)
    }
}

impl Drop for PlanTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn plan_attachment(name: &str, mime_type: &str, bytes: &[u8]) -> PromptAttachment {
    use base64::Engine;
    PromptAttachment {
        name: name.to_string(),
        mime_type: mime_type.to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

fn capable_catalog() -> PiCatalog {
    catalog_from_capture(PI_MODELS_CAPTURE)
}

#[test]
fn pi_delivery_follows_the_current_model_tri_state() {
    // Paseo's `piModelSupportsImageInput` through the tri-state this
    // daemon already keeps: `Supported` frames bytes, `Unsupported` AND
    // `Absent` keep the path line.
    let catalog = capable_catalog();
    assert_eq!(
        pi_delivery(&catalog, Some("minimax-m3")),
        super::super::ImageDelivery::StaticImageBlock,
        "a model that declared image frames bytes"
    );
    assert_eq!(
        pi_delivery(&catalog, Some("deepseek-v4-flash")),
        super::super::ImageDelivery::PathLine,
        "a model that declared no image keeps the path line"
    );
    assert_eq!(
        pi_delivery(&catalog, Some("no-such-model")),
        super::super::ImageDelivery::PathLine,
        "an unknown model is Absent: never an attempt"
    );
    assert_eq!(
        pi_delivery(&catalog, None),
        super::super::ImageDelivery::PathLine,
        "no current model is Absent: never an attempt"
    );
}

#[test]
fn a_capable_pi_model_plans_an_image_entry_and_no_path_line() {
    // The raster becomes one `images[]` entry; the text is the bare user
    // text, with no path line. The frame carries the field; the
    // text-only frame omits it.
    let temp = PlanTempDir::new("capable");
    let store = AttachmentStore::new(&temp.0);
    let catalog = capable_catalog();
    // A container the walk accepts but changes: what the entry carries
    // must be the stripped bytes, never the wire bytes.
    let sent = png_with_text_chunk();
    let kept = clean_png(0x01);
    assert_ne!(
        sent, kept,
        "the fixture must actually carry something that leaves"
    );
    let plan = plan_pi_prompt(
        &store,
        "pi-plan-capable",
        "describe this",
        &[plan_attachment("photo.png", "image/png", &sent)],
        &catalog,
        Some("minimax-m3"),
    )
    .expect("materialized")
    .expect("a capable model plans an entry");
    assert_eq!(plan.fallback_text, "describe this", "no path line");
    assert_eq!(carried_pi_mime_types(Some(&plan)), vec!["image/png"]);
    assert_eq!(plan.images.len(), 1);
    {
        use base64::Engine;
        assert_eq!(
            plan.images[0].data_base64,
            base64::engine::general_purpose::STANDARD.encode(&kept),
            "the entry carries the stripped bytes"
        );
    }
    // The exact entry shape, pinned literally: flat, capital-T
    // `mimeType` — not Claude's nested `source`/`media_type`.
    let entry = pi_image_entry(&plan.images[0].mime_type, &plan.images[0].data_base64);
    assert_eq!(entry["type"], "image");
    assert_eq!(entry["mimeType"], "image/png");
    assert!(entry["data"].as_str().is_some());
    assert!(entry.get("source").is_none(), "no Claude nesting");
    assert!(entry.get("media_type").is_none(), "no Claude key");
    let frame = pi_prompt_frame("p-1", &plan.fallback_text, &plan.images);
    assert_eq!(frame["message"], "describe this");
    let images = frame["images"].as_array().expect("images array");
    assert_eq!(images.len(), 1);
    assert_eq!(images[0]["mimeType"], "image/png");
    let bare = pi_prompt_frame("p-test", "describe this", &[]);
    assert_eq!(bare["message"], "describe this");
    assert!(
        bare.get("images").is_none(),
        "text-only omits the field, never an empty array"
    );
}

#[test]
fn an_incapable_pi_model_keeps_the_path_line_and_plans_nothing() {
    // `deepseek-v4-flash` declares text only: the safe answer is no plan,
    // so the caller takes the legacy path-line write.
    let temp = PlanTempDir::new("incapable");
    let store = AttachmentStore::new(&temp.0);
    let catalog = capable_catalog();
    let plan = plan_pi_prompt(
        &store,
        "pi-plan-incapable",
        "describe this",
        &[plan_attachment("photo.png", "image/png", &clean_png(0x11))],
        &catalog,
        Some("deepseek-v4-flash"),
    )
    .expect("materialized");
    assert!(plan.is_none(), "an incapable model plans nothing");
}

#[test]
fn an_unknown_pi_model_keeps_the_path_line_and_plans_nothing() {
    // Absent (unknown model, or no current model): silence is not
    // consent, so the path line is the safe answer. Unknown never means
    // yes — this is the third state the tri-state exists to keep apart
    // from a refusal.
    let temp = PlanTempDir::new("unknown");
    let store = AttachmentStore::new(&temp.0);
    let catalog = capable_catalog();
    for model in [Some("no-such-model"), None] {
        let plan = plan_pi_prompt(
            &store,
            "pi-plan-unknown",
            "describe this",
            &[plan_attachment("photo.png", "image/png", &clean_png(0x12))],
            &catalog,
            model,
        )
        .expect("materialized");
        assert!(plan.is_none(), "an unknown model plans nothing");
    }
}

#[test]
fn an_svg_keeps_its_path_line_beside_pi_image_entries() {
    // A mixed prompt carries both: the raster as an entry, the SVG as a
    // path line in the text.
    let temp = PlanTempDir::new("mixed");
    let store = AttachmentStore::new(&temp.0);
    let catalog = capable_catalog();
    let source = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
    let plan = plan_pi_prompt(
        &store,
        "pi-plan-mixed",
        "logo and photo",
        &[
            plan_attachment("photo.png", "image/png", &clean_png(0x13)),
            plan_attachment("drawing.svg", "image/svg+xml", source),
        ],
        &catalog,
        Some("minimax-m3"),
    )
    .expect("materialized")
    .expect("the raster plans an entry");
    assert_eq!(carried_pi_mime_types(Some(&plan)), vec!["image/png"]);
    assert!(
        plan.fallback_text
            .starts_with("logo and photo\n\n[Image available at: "),
        "{}",
        plan.fallback_text
    );
    assert!(
        plan.fallback_text.ends_with(".svg]"),
        "{}",
        plan.fallback_text
    );
    assert!(
        !plan.fallback_text.contains(".png]"),
        "the raster left no path line: {}",
        plan.fallback_text
    );
    let frame = pi_prompt_frame("p-2", &plan.fallback_text, &plan.images);
    assert!(frame["message"].as_str().expect("text").ends_with(".svg]"));
    assert_eq!(frame["images"].as_array().expect("array").len(), 1);
}

#[test]
fn a_frame_without_entries_is_the_text_only_frame() {
    // The static route frames every prompt it plans through this builder,
    // including one whose entries are all path lines. With no entry it has
    // to be the frame the writer builds inline, byte for byte.
    assert_eq!(
        pi_prompt_frame("p-test", "describe this", &[]),
        serde_json::json!({"id": "p-test", "type": "prompt", "message": "describe this"})
    );
}

#[test]
fn pi_steer_frame_is_byte_exact_and_has_literal_empty_images() {
    // The frame `request` writes for a steer: the id it mints, the command,
    // then the fields — with the literal empty `images` array Pi's own
    // `steer(text, images)` sends, deliberately unlike `pi_prompt_frame`,
    // which omits `images` when it carries none.
    assert_eq!(
        serde_json::to_vec(&pi_control_frame("c-1", "steer", pi_steer_fields("hello")))
            .expect("frame"),
        br#"{"id":"c-1","type":"steer","text":"hello","images":[]}"#
    );
    assert!(pi_prompt_frame("p-test", "hello", &[])
        .get("images")
        .is_none());
}

/// A fake Pi that answers every control frame with a response naming the
/// frame's own id, echoing the raw line it read. Pi answers commands this
/// way; what differs between builds is `success` and the error it spells.
fn fake_pi(
    script: &str,
) -> (
    std::process::Child,
    Arc<Mutex<Option<ChildStdin>>>,
    std::io::BufReader<std::process::ChildStdout>,
) {
    let mut child = node_command()
        .args(["-e", script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("{}", node_unavailable("Pi steer round-trip test", &error)));
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
    (child, stdin, stdout)
}

/// A Pi whose build has no `steer` command, as its control protocol
/// answers one.
const FAKE_PI_UNKNOWN_STEER: &str = r#"
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    process.stdout.write(
      JSON.stringify({
        id: frame.id,
        type: "response",
        success: false,
        error: "Unknown command: steer",
        received: line,
      }) + "\n"
    );
  }
});
"#;

/// A Pi that takes the steer, and echoes the frame it took.
const FAKE_PI_STEERS: &str = r#"
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    process.stdout.write(
      JSON.stringify({
        id: frame.id,
        type: "response",
        success: true,
        received: line,
      }) + "\n"
    );
  }
});
"#;

/// The pieces one round-trip test drives.
struct AnsweringPi {
    child: std::process::Child,
    control: Arc<PiControl>,
    answers: Arc<Mutex<Vec<serde_json::Value>>>,
    reader: std::thread::JoinHandle<()>,
}

impl AnsweringPi {
    /// Stop the fake Pi, join its reader, and hand back what that reader
    /// read: each answer, in arrival order.
    fn answers(mut self) -> Vec<serde_json::Value> {
        let answers = self.answers.lock().expect("answers").clone();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.reader.join();
        answers
    }
}

/// A fake Pi whose answers are delivered the way the real client delivers
/// them: a reader thread turns each stdout line into a pending response, so
/// the round-trip is correlated by id instead of timing out. Answers are
/// recorded before delivery, so a test can assert on them the moment the
/// steer returns.
fn fake_pi_answering(script: &str) -> AnsweringPi {
    let (child, stdin, stdout) = fake_pi(script);
    let control = Arc::new(PiControl::new(stdin, Arc::new(AtomicU64::new(1))));
    let answers = Arc::new(Mutex::new(Vec::new()));
    let reader_control = Arc::clone(&control);
    let reader_answers = Arc::clone(&answers);
    let reader = std::thread::spawn(move || {
        let mut stdout = stdout;
        let mut line = String::new();
        loop {
            line.clear();
            match std::io::BufRead::read_line(&mut stdout, &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let Ok(answer) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if let Ok(mut answers) = reader_answers.lock() {
                answers.push(answer.clone());
            }
            let _ = reader_control.deliver(&answer);
        }
    });
    AnsweringPi {
        child,
        control,
        answers,
        reader,
    }
}

/// A reader with no child behind it, for the tests that drive the reader's
/// own ends — the end of the control channel and the id-correlated delivery
/// — rather than a spawned program. The extension path is empty, so nothing
/// on disk is touched.
fn reader_with_control(control: Arc<PiControl>) -> PiReader {
    let stdin: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(None));
    PiReader::new(
        Vec::new(),
        SessionEvent::SessionManifest {
            provider_id: Some("pi".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: None,
        },
        super::PermissionBroker::for_test(Arc::new(|_, _| Ok(()))),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(AtomicU64::new(1)),
        control,
        stdin,
        Arc::new(std::sync::atomic::AtomicBool::new(true)),
    )
}

#[test]
fn the_control_channel_ending_wakes_every_waiter() {
    // A2-02: the reader thread is what delivers answers, so when the child's
    // output ends no answer can arrive for anyone still waiting. Left
    // registered, each waiter would sit out the whole response timeout for a
    // reply the reader can no longer deliver — the steer's own timeout is
    // fifteen seconds of that.
    let stdin: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(None));
    let control = Arc::new(PiControl::new(
        Arc::clone(&stdin),
        Arc::new(AtomicU64::new(1)),
    ));
    let response = {
        // Registered exactly as `begin` registers one, without a child to
        // write to: what this pins is the wait.
        let (sender, receiver) = std::sync::mpsc::channel();
        control
            .pending
            .lock()
            .expect("pending")
            .insert("c-1".to_string(), sender);
        receiver
    };
    let mut reader = reader_with_control(Arc::clone(&control));
    let runtime = Arc::new(SessionRuntime::new());

    reader.finish(&runtime);

    let answer = response
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("the waiter is woken by the end of the channel");
    let message = answer.expect_err("no answer can arrive: the channel is over");
    assert!(message.contains("control channel"), "{message}");
    assert!(
        control.pending.lock().expect("pending").is_empty(),
        "and the table is left with no waiter to wake twice"
    );
}

#[test]
fn a_control_response_for_an_id_nobody_waits_for_is_ignored() {
    // A2-02: the id space carries answers that are not this control's — to
    // commands no waiter registered, and to requests whose waiter already
    // timed out. An unknown id is nothing to deliver, not an error and not a
    // panic, and it must not disturb the waiter that *is* registered.
    let stdin: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(None));
    let control = PiControl::new(Arc::clone(&stdin), Arc::new(AtomicU64::new(1)));
    let (sender, receiver) = std::sync::mpsc::channel();
    control
        .pending
        .lock()
        .expect("pending")
        .insert("c-1".to_string(), sender);

    assert!(
        !control.deliver(&serde_json::json!({
            "id": "c-404",
            "type": "response",
            "success": true,
        })),
        "no waiter holds that id"
    );
    assert!(
        !control.deliver(&serde_json::json!({ "type": "response", "success": true })),
        "a response with no id at all names nobody"
    );

    assert!(control.deliver(&serde_json::json!({
        "id": "c-1",
        "type": "response",
        "success": true,
    })));
    let answer = receiver
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("the registered waiter still takes its own answer");
    assert_eq!(answer.expect("an answer, not a failure")["id"], "c-1");
}

/// The refusal Pi sends for a command its build has no handler for, in the
/// spelling the test passes (A2-11): the script is `FAKE_PI_UNKNOWN_STEER`
/// with its error message replaced, so the two cases differ in nothing else.
fn fake_pi_refusing(error: &str) -> String {
    FAKE_PI_UNKNOWN_STEER.replace("Unknown command: steer", error)
}

#[test]
fn a_pi_steer_refusal_is_recognised_whatever_its_case() {
    // A2-11: what makes the steer unavailable is the app-server saying it
    // has no such command, not the exact spelling. A case-sensitive match
    // turns `unknown command: steer` into an `Err` — "the steer's fate is
    // unknown" — for a build that said exactly what happened.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let pi = fake_pi_answering(&fake_pi_refusing("unknown command: steer"));
    let mut steerer = PiSteerer {
        control: Arc::clone(&pi.control),
    };
    assert!(
        matches!(
            steer_through_the_turn(&mut steerer, "turn left"),
            Some(Ok(false))
        ),
        "a lower-case refusal is the same refusal"
    );
    let answers = pi.answers();
    assert_eq!(answers.len(), 1, "one answer, read by the reader");
    assert_eq!(
        answers[0]["error"],
        serde_json::json!("unknown command: steer")
    );
}

#[test]
fn a_pi_that_does_not_know_the_steer_command_is_unavailable_rather_than_steered() {
    // The answer Pi sends is the one that decides: a write alone reports
    // that the bytes reached the pipe, which a build without `steer` also
    // does before it rejects the command (the fix-pass rule for S4-01 on
    // this provider). The daemon has to read the answer, so the steer goes
    // through the id-correlated round-trip.
    // A2-13: the fake Pi is a `node` script, so this skips where there is no
    // node rather than failing there.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let pi = fake_pi_answering(FAKE_PI_UNKNOWN_STEER);
    let mut steerer = PiSteerer {
        control: Arc::clone(&pi.control),
    };
    assert!(
        matches!(
            steer_through_the_turn(&mut steerer, "turn left"),
            Some(Ok(false))
        ),
        "an unknown command is unavailable, not steered"
    );
    let answers = pi.answers();
    assert_eq!(
        answers.len(),
        1,
        "one answer, read by the delivering reader"
    );
    assert_eq!(
        answers[0]["error"],
        serde_json::json!("Unknown command: steer"),
        "the refusal is what made the steer unavailable"
    );
}

#[test]
fn a_pi_that_takes_the_steer_is_steered_with_the_frame_the_round_trip_wrote() {
    // A2-13: the fake Pi is a `node` script, so this skips where there is no
    // node rather than failing there.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let pi = fake_pi_answering(FAKE_PI_STEERS);
    let mut steerer = PiSteerer {
        control: Arc::clone(&pi.control),
    };
    assert!(matches!(
        steer_through_the_turn(&mut steerer, "hello"),
        Some(Ok(true))
    ));
    let answers = pi.answers();
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["success"], serde_json::json!(true));
    // The frame as the child received it, byte for byte: `begin` mints the
    // id that correlates the answer, and the steer's own fields follow it.
    assert_eq!(
        answers[0]["received"].as_str(),
        Some(r#"{"id":"c-1","type":"steer","text":"hello","images":[]}"#)
    );
}

#[test]
fn steering_a_slash_input_is_refused_as_paseo_refuses_it() {
    // Paseo `pi/agent.ts:1417-1419`: "Pi rejects steer RPCs that are
    // extension commands", so a `/…` input is never steered — it answers
    // `Ok(false)`, the refusal that sends the caller down its pre-existing
    // interrupt-and-replace, where the text can run directly. No child is
    // needed: the refusal must happen *before* a frame is written, which is
    // what the childless control proves — a write attempt would error.
    let stdin: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(None));
    let mut steerer = PiSteerer {
        control: Arc::new(PiControl::new(stdin, Arc::new(AtomicU64::new(1)))),
    };
    assert!(
        matches!(
            steer_through_the_turn(&mut steerer, "/goal x"),
            Some(Ok(false))
        ),
        "a slash input is refused for steering, not written and not an error"
    );
    assert!(
        matches!(
            steer_through_the_turn(&mut steerer, "plain text"),
            Some(Err(_))
        ),
        "a non-slash steer keeps the old road: with no child its write fails as a transport error, which is not the refusal"
    );
}

#[test]
fn only_the_reply_that_answers_the_live_request_becomes_the_command_list() {
    // Paseo drops a response no live request waits for
    // (`jsonl-rpc-process.ts:157-163, 283-292`); ours must too — and harder:
    // the row is the journal's copy, so a reply that answered nothing must
    // reach neither the transcript nor the replay source, or a reattach would
    // derive a list nobody asked for (review A5-2 #1). The registration below
    // is what `commands::begin_get_commands` puts in the table; the two later
    // replies are an id nobody holds and the same id once its waiter is gone.
    use crate::journal::{new_session_record, Journal};

    let session_id = "pi-commands-correlated";
    let dir = crate::test_dirs::test_temp_dir("devboule-pi-commands-correlated");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            devboule_protocol::SessionKind::Pi,
            "Agent",
        ))
        .expect("upsert");
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
    }
    let stdin: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(None));
    let control = Arc::new(PiControl::new(
        Arc::clone(&stdin),
        Arc::new(AtomicU64::new(1)),
    ));
    let mut reader = reader_with_control(Arc::clone(&control));
    let (sender, held) = std::sync::mpsc::channel();
    control
        .pending
        .lock()
        .expect("pending")
        .insert("c-1".to_string(), sender);
    let reply = |id: &str| {
        serde_json::from_str::<serde_json::Value>(&format!(
            r#"{{"id":"{id}","type":"response","command":"get_commands","success":true,"data":{{"commands":[{{"name":"goal","description":"Set the session goal","source":"extension","input":{{"hint":"<objective>"}}}}]}}}}"#
        ))
        .expect("recorded reply")
    };

    // Dispatch before any observer exists — the shared backlog holds the
    // correlated reply's copy, the journal its row, and a fresh attach is
    // given both and must collapse them to one (the seam's own contract,
    // the shape `a_turn_end_line_delivers_each_event_exactly_once` proves).
    // A dropped reply would add a second list either way it was wrong:
    // published into the backlog, or journaled for replay.
    reader
        .dispatch_value(reply("c-1"), &runtime)
        .expect("the correlated reply dispatches");
    held.recv_timeout(std::time::Duration::from_secs(2))
        .expect("the waiter's own channel took the reply")
        .expect("the reply itself is an answer, not a channel failure");
    reader
        .dispatch_value(reply("c-404"), &runtime)
        .expect("the foreign reply dispatches");
    reader
        .dispatch_value(reply("c-1"), &runtime)
        .expect("the late reply dispatches");
    journal.flush().expect("flush");

    let conn = crate::session::ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let mut listed = Vec::new();
    loop {
        let batch = conn.pull_events();
        if batch.is_empty() {
            break;
        }
        for pending in &batch {
            if let SessionEvent::AvailableCommands { commands } = &pending.envelope.event {
                listed.push(commands.clone());
            }
        }
    }
    assert_eq!(
        listed.len(),
        1,
        "one live request's reply becomes one list — nothing from the dropped replies"
    );
    let entries = listed[0]
        .iter()
        .map(|command| (command.name.as_str(), command.hint.as_deref()))
        .collect::<Vec<_>>();
    assert_eq!(
        entries,
        [
            ("compact", Some("[instructions]")),
            ("autocompact", Some("[on|off|toggle]")),
            ("goal", Some("<objective>")),
        ],
        "the two seeds with their hints, and the reply's own hint kept"
    );

    drop(runtime);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_static_route_answers_for_the_model_current_at_prompt_time() {
    // The route reads the live catalog rather than a copy taken at spawn:
    // a model switched since then must not be answered for with the inputs
    // the old model declared. Both directions are pinned here.
    let temp = PlanTempDir::new("route-model");
    let store = AttachmentStore::new(&temp.0);
    let catalog = Arc::new(Mutex::new(capable_catalog()));
    let route = PiStaticPrompt::new(
        Arc::new(Mutex::new(None)),
        Arc::new(AtomicU64::new(1)),
        Arc::clone(&catalog),
    );
    let attachment = plan_attachment("photo.png", "image/png", &clean_png(0x41));
    catalog.lock().expect("catalog").current_model_id = Some("minimax-m3".to_string());
    let planned = route
        .plan_prompt(
            &store,
            "pi-route",
            "describe this",
            std::slice::from_ref(&attachment),
        )
        .expect("planned")
        .expect("a model that declared image plans a frame");
    assert_eq!(planned.text(), "describe this", "no path line");
    // The same route on a text-only model declines, and the caller takes
    // the legacy write.
    catalog.lock().expect("catalog").current_model_id = Some("deepseek-v4-flash".to_string());
    assert!(route
        .plan_prompt(
            &store,
            "pi-route",
            "describe this",
            std::slice::from_ref(&attachment)
        )
        .expect("planned")
        .is_none());
}

#[test]
fn a_jpeg_stays_a_jpeg_in_the_pi_entry() {
    // The label `materialize` checked against the sniffed container is
    // the label the entry carries.
    const EXIF_JPEG_VECTOR: &str =
        "a jpeg whose APP1 holds EXIF, between a kept JFIF APP0 and a kept ICC APP2";
    let temp = PlanTempDir::new("jpeg");
    let store = AttachmentStore::new(&temp.0);
    let catalog = capable_catalog();
    let sent = vector_input(EXIF_JPEG_VECTOR);
    let kept = vector_output(EXIF_JPEG_VECTOR);
    assert_ne!(sent, kept, "the vector must actually strip something");
    let plan = plan_pi_prompt(
        &store,
        "pi-plan-jpeg",
        "describe this",
        &[plan_attachment("photo.jpg", "image/jpeg", &sent)],
        &catalog,
        Some("minimax-m3"),
    )
    .expect("materialized")
    .expect("a JPEG plans an entry");
    assert_eq!(plan.fallback_text, "describe this");
    assert_eq!(carried_pi_mime_types(Some(&plan)), vec!["image/jpeg"]);
    {
        use base64::Engine;
        assert_eq!(
            plan.images[0].data_base64,
            base64::engine::general_purpose::STANDARD.encode(&kept),
            "stripped JPEG bytes, JPEG label"
        );
    }
}
#[cfg(test)]
mod delivery_tests {
    use super::{fake_pi_answering, PiCatalog, PiControl, PiSwitcher};
    use crate::session::ModelSwitcher;
    use devboule_protocol::SessionModelEffort;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::{Arc, Mutex};

    // Re-exported through the tests module's own namespace where they are
    // already in scope; the two that are not come straight from home.
    use crate::session::pi_client::{PiInputKinds, PiModel};

    /// A fake Pi that answers every control command with success and, for
    /// `get_available_thinking_levels`, a real level list. Each answer echoes
    /// the request line back, so the test can assert on the *requests* — the
    /// wire is what the delivery is.
    const FAKE_PI_DELIVERS: &str = r#"
    let buffered = "";
    process.stdin.on("data", (chunk) => {
      buffered += chunk;
      let index;
      while ((index = buffered.indexOf("\n")) >= 0) {
        const line = buffered.slice(0, index);
        buffered = buffered.slice(index + 1);
        const frame = JSON.parse(line);
        const answer = { id: frame.id, type: "response", success: true, received: line };
        if (frame.type === "get_available_thinking_levels") {
          answer.data = { levels: ["high", "low"] };
        }
        process.stdout.write(JSON.stringify(answer) + "\n");
      }
    });
    "#;

    /// The delivery is the wire: a delivered model and thinking option are
    /// the `set_model` and `set_thinking_level` requests this client writes
    /// after the handshake, in that order, with the delivered values. A
    /// delivery that skips a request is not a delivery.
    #[test]
    fn delivery_writes_set_model_and_set_thinking_level_on_the_wire() {
        let pi = fake_pi_answering(FAKE_PI_DELIVERS);
        let catalog = PiCatalog {
            models: HashMap::from([(
                "pi-model".to_string(),
                PiModel {
                    name: "Pi Model".to_string(),
                    provider: Some("pi-provider".to_string()),
                    context_tokens: None,
                    efforts: Some(vec![
                        SessionModelEffort {
                            id: "high".to_string(),
                            label: "High".to_string(),
                            description: None,
                            default: Some(true),
                        },
                        SessionModelEffort {
                            id: "low".to_string(),
                            label: "Low".to_string(),
                            description: None,
                            default: None,
                        },
                    ]),
                    input: PiInputKinds::default(),
                },
            )]),
            current_model_id: Some("pi-model".to_string()),
            current_provider: Some("pi-provider".to_string()),
            current_effort: Some("high".to_string()),
            current_levels: vec!["high".to_string()],
        };
        let switcher = PiSwitcher {
            control: Arc::clone(&pi.control),
            catalog: Arc::new(Mutex::new(catalog)),
            mode_id: Arc::new(Mutex::new("ask".to_string())),
            permission_extension_active: Arc::new(AtomicBool::new(false)),
        };
        switcher
            .set_model(Some("pi-model"), Some("low"))
            .expect("delivered");
        let answers = pi.answers();

        let received: Vec<String> = answers
            .iter()
            .filter_map(|answer| answer["received"].as_str().map(|line| line.to_string()))
            .collect();
        let commands: Vec<String> = received
            .iter()
            .filter_map(|line| {
                serde_json::from_str::<serde_json::Value>(line)
                    .ok()?
                    .get("type")
                    .and_then(|kind| kind.as_str())
                    .map(str::to_string)
            })
            .collect();
        assert_eq!(
            commands,
            vec![
                "set_model".to_string(),
                "get_available_thinking_levels".to_string(),
                "set_thinking_level".to_string()
            ],
            "the delivery writes model then level, on the wire: {received:?}"
        );
        assert!(
            received[0].contains("pi-model"),
            "the set_model request carries the delivered model: {}",
            received[0]
        );
        assert!(
            received[2].contains("low"),
            "the set_thinking_level request carries the delivered level: {}",
            received[2]
        );
    }

    /// The model-absence refusal fires before anything is written: a provider
    /// that publishes no models cannot deliver any choice, and that is a
    /// different sentence from "the named model is not in the list". The
    /// control's stdin is closed — if the test reaches the wire it fails
    /// loudly instead of passing quietly.
    #[test]
    fn a_pi_model_against_an_empty_catalog_is_the_absence_refusal() {
        let stdin: Arc<Mutex<Option<std::process::ChildStdin>>> = Arc::new(Mutex::new(None));
        let switcher = PiSwitcher {
            control: Arc::new(PiControl::new(stdin, Arc::new(AtomicU64::new(1)))),
            catalog: Arc::new(Mutex::new(PiCatalog {
                models: HashMap::new(),
                current_model_id: None,
                current_provider: None,
                current_effort: None,
                current_levels: Vec::new(),
            })),
            mode_id: Arc::new(Mutex::new("ask".to_string())),
            permission_extension_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let error = switcher
            .set_model(Some("pi-model"), None)
            .expect_err("an empty catalog cannot deliver a model");
        assert!(
            error.message.contains("publishes no models"),
            "the absence sentence: {}",
            error.message
        );
        assert!(
            !error.message.contains("is not in get_available_models"),
            "the two sentences must stay distinct: {}",
            error.message
        );
    }

    /// The tick contradiction, walked over pi's own closed mode
    /// vocabulary plus the broker table (the R2a audit's F9 — this
    /// refusal had no test at all): for every mode pi can start in, a
    /// tick is admitted exactly when the daemon's broker answers that
    /// mode, and refused with the contradiction sentence otherwise.
    #[test]
    fn a_pi_auto_accept_tick_over_an_asking_mode_is_refused() {
        fn delivery(mode: &str, tick: bool) -> crate::profile_delivery::ProfileDelivery {
            let mut delivery = crate::profile_delivery::ProfileDelivery::for_child(
                mode,
                "pi-model",
                None,
                &serde_json::Map::new(),
            );
            delivery.auto_accept = tick;
            delivery
        }
        let validate_delivery = super::super::validate_delivery;

        // The closed table intersects pi's own vocabulary at exactly
        // `bypass`: that intersection is the route a tick rides, so it
        // is asserted, not assumed.
        let broker = crate::provider_catalog::auto_answered_modes();
        assert!(
            broker.contains(&"bypass"),
            "bypass is the pi mode the daemon's broker answers"
        );
        let vocabulary = ["bypass", "ask"];
        for mode_id in vocabulary {
            if crate::provider_catalog::mode_is_auto_answered(mode_id) {
                validate_delivery(&delivery(mode_id, true)).unwrap_or_else(|error| {
                    panic!("{mode_id} answers its own prompts: {}", error.message)
                });
            } else {
                let error = validate_delivery(&delivery(mode_id, true))
                    .expect_err("a tick over an asking mode is the contradiction");
                assert!(
                    error.message.contains("contradict") && error.message.contains(mode_id),
                    "the refusal names both halves for {mode_id}: {}",
                    error.message
                );
            }
            validate_delivery(&delivery(mode_id, false))
                .unwrap_or_else(|error| panic!("{mode_id} without the tick: {}", error.message));
        }

        // A mode that is not pi's at all is refused with its own
        // sentence, tick or no tick.
        let error =
            validate_delivery(&delivery("default", false)).expect_err("an unknown mode is refused");
        assert!(
            error.message.contains("is not available"),
            "the mode sentence: {}",
            error.message
        );
    }
}

/// A fake Pi that logs every command it is asked and answers each with
/// the id-correlated success the control protocol spells, with real
/// levels for `get_available_thinking_levels`.
const FAKE_PI_DELIVERY_ANSWERS: &str = r#"
const fs = require("fs");
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    fs.appendFileSync(process.env.DEVBOULE_FAKE_PI_LOG, frame.type + "\n");
    const answer = { id: frame.id, type: "response", success: true, received: line };
    if (frame.type === "get_available_thinking_levels") {
      answer.data = { levels: ["high", "low"] };
    }
    process.stdout.write(JSON.stringify(answer) + "\n");
  }
});
"#;

/// The same fake, refusing every `set_model` the way a build that will
/// not take the model answers: `success: false`, with an error.
const FAKE_PI_DELIVERY_REFUSES: &str = r#"
const fs = require("fs");
// An open listener keeps this process alive when its stdin closes, so "the
// child exited" after the teardown can only mean the kill did it — a
// teardown that merely closed the pipe cannot satisfy the assertion (the
// re-audit's P3-6).
require("net").createServer().listen(0, "127.0.0.1");
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    fs.appendFileSync(process.env.DEVBOULE_FAKE_PI_LOG, frame.type + "\n");
    const answer = { id: frame.id, type: "response",
      success: frame.type !== "set_model",
      error: "unknown model", received: line };
    if (frame.type === "get_available_thinking_levels") {
      answer.data = { levels: ["high", "low"] };
    }
    process.stdout.write(JSON.stringify(answer) + "\n");
  }
});
"#;

/// The gated fake: every request is logged the moment it arrives (so a
/// test can see the delivery is in flight) but the `set_model` answer is
/// held until a gate file appears. It is the P2-1 window made
/// deterministic: the reader is live, the rpc is on the wire, the answer
/// never comes.
const FAKE_PI_DELIVERY_GATED: &str = r#"
const fs = require("fs");
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    fs.appendFileSync(process.env.DEVBOULE_FAKE_PI_LOG, frame.type + "\n");
    if (frame.type === "set_model" && !fs.existsSync(process.env.DEVBOULE_FAKE_PI_GATE)) {
      const held = setInterval(() => {
        if (fs.existsSync(process.env.DEVBOULE_FAKE_PI_GATE)) {
          clearInterval(held);
          answer(frame);
        }
      }, 20);
      return;
    }
    answer(frame);
  }
});
function answer(frame) {
  const answer = { id: frame.id, type: "response", success: true };
  if (frame.type === "get_available_thinking_levels") {
    answer.data = { levels: ["high", "low"] };
  }
  process.stdout.write(JSON.stringify(answer) + "\n");
}
"#;

/// The resume wiring test's fake: it writes its own launch line where the
/// test reads it, then serves the handshake so `spawn_process_resuming`
/// completes. `get_state` answers the id the resume asked for, the way a
/// real pi answers a resolved `--session`.
const FAKE_PI_RESUME_HANDSHAKE: &str = r#"
const fs = require("fs");
fs.appendFileSync(process.env.DEVBOULE_FAKE_PI_ARGV_LOG, JSON.stringify(process.argv) + "\n");
// The injected permission extension's readiness announce, which a child in
// the `ask` mode the resume road starts in must produce before the third
// handshake response lands.
process.stdout.write(JSON.stringify({ type: "extension_ui_request", method: "notify", message: "devboule-permission-channel" }) + "\n");
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    fs.appendFileSync(process.env.DEVBOULE_FAKE_PI_LOG, frame.type + "\n");
    let answer = { success: true };
    if (frame.type === "get_state") {
      answer.data = { sessionId: process.env.DEVBOULE_FAKE_PI_SESSION, model: { id: "pi-model", provider: "pi-provider" }, thinkingLevel: "high" };
    } else if (frame.type === "get_available_models") {
      answer.data = { models: [
        { id: "pi-model", name: "Pi Model", provider: "pi-provider", thinkingLevelMap: { high: {}, low: {} } }
      ] };
    } else if (frame.type === "get_available_thinking_levels") {
      answer.data = { levels: ["high", "low"] };
    }
    answer.id = frame.id;
    answer.type = "response";
    process.stdout.write(JSON.stringify(answer) + "\n");
  }
});
"#;

/// A fake that serves the real `spawn_process` handshake — `get_state`,
/// `get_available_models`, `get_available_thinking_levels` — and then
/// answers whatever else comes, logging every frame. This is the fake
/// the spawn seam is tested with (the re-audit's P2-3).
const FAKE_PI_SPAWN_HANDSHAKE: &str = r#"
const fs = require("fs");
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index);
    buffered = buffered.slice(index + 1);
    const frame = JSON.parse(line);
    fs.appendFileSync(process.env.DEVBOULE_FAKE_PI_LOG, frame.type + "\n");
    let answer = { success: true };
    if (frame.type === "get_state") {
      answer.data = { model: { id: "pi-model", provider: "pi-provider" }, thinkingLevel: "high" };
    } else if (frame.type === "get_available_models") {
      answer.data = { models: [
        { id: "pi-model", name: "Pi Model", provider: "pi-provider", thinkingLevelMap: { high: {}, low: {} } }
      ] };
    } else if (frame.type === "get_available_thinking_levels") {
      answer.data = { levels: ["high", "low"] };
    }
    answer.id = frame.id;
    answer.type = "response";
    process.stdout.write(JSON.stringify(answer) + "\n");
  }
});
"#;

/// The lifecycle the R2a audit's F1 convicted: a profile delivery for pi
/// is an awaited control rpc, and the only code that can deliver its
/// answer is the session reader thread `start_spawned_session` starts.
/// These tests drive that real ordering — the `SpawnedSession` is
/// assembled the way `spawn_process` assembles it, the delivery travels
/// as [`pending_pi_delivery`] packages it, and **no fixture starts a
/// reader for the client**: the deliverer under test is the production
/// one. A regression that runs the delivery before that reader exists
/// cannot be answered here except by the fifteen-second timeout and the
/// refusal that follows — which is exactly the production failure.
mod lifecycle_tests {
    use super::super::PtyCommand;
    use super::super::{
        pending_pi_delivery, spawn_process, spawn_process_resuming, PiCatalog, PiControl,
        PiInputKinds, PiKiller, PiModel, PiReader, PiStaticPrompt, PiStderr, PiStdout, PiSwitcher,
        PiWriter,
    };
    use crate::process_tree::JobObject;
    use crate::profile_delivery::ProfileDelivery;
    use crate::server::ServerState;
    use crate::session::permission_broker::PermissionBroker;
    use crate::session::{start_spawned_session, SpawnedSession, StdioWaitableChild};
    use devboule_protocol::{OwnerId, SessionEvent, SessionModelEffort};
    use std::collections::HashMap;
    use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// The delivery test's model, verbatim: one model, two levels.
    fn delivery_catalog() -> PiCatalog {
        PiCatalog {
            models: HashMap::from([(
                "pi-model".to_string(),
                PiModel {
                    name: "Pi Model".to_string(),
                    provider: Some("pi-provider".to_string()),
                    context_tokens: None,
                    efforts: Some(vec![
                        SessionModelEffort {
                            id: "high".to_string(),
                            label: "High".to_string(),
                            description: None,
                            default: Some(true),
                        },
                        SessionModelEffort {
                            id: "low".to_string(),
                            label: "Low".to_string(),
                            description: None,
                            default: None,
                        },
                    ]),
                    input: PiInputKinds::default(),
                },
            )]),
            current_model_id: Some("pi-model".to_string()),
            current_provider: Some("pi-provider".to_string()),
            current_effort: Some("high".to_string()),
            current_levels: vec!["high".to_string()],
        }
    }

    fn pi_delivery() -> ProfileDelivery {
        ProfileDelivery::for_child("ask", "pi-model", Some("low"), &serde_json::Map::new())
    }

    /// A fake Pi on real pipes. Stdout is deliberately **not** wrapped in
    /// a reader thread here — the `PiStdout` the session reader will
    /// drain is built inside `spawned_session`, and nothing else reads
    /// the child.
    struct SpawnedPi {
        process: Arc<Mutex<Child>>,
        stdout: ChildStdout,
        stdin: Arc<Mutex<Option<ChildStdin>>>,
        stderr: ChildStderr,
        next_id: Arc<AtomicU64>,
        catalog: Arc<Mutex<PiCatalog>>,
    }

    fn spawned_pi(script: &str, log: &std::path::Path) -> SpawnedPi {
        spawned_pi_with_env(script, log, &[])
    }

    fn spawned_pi_with_env(
        script: &str,
        log: &std::path::Path,
        extra: &[(&str, String)],
    ) -> SpawnedPi {
        let mut child = Command::new("node")
            .arg("-e")
            .arg(script)
            .env("DEVBOULE_FAKE_PI_LOG", log)
            .envs(extra.iter().map(|(key, value)| (*key, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| {
                panic!(
                    "{}",
                    super::node_unavailable("Pi delivery lifecycle test", &error)
                )
            });
        let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        SpawnedPi {
            process: Arc::new(Mutex::new(child)),
            stdout,
            stdin,
            stderr,
            next_id: Arc::new(AtomicU64::new(1)),
            catalog: Arc::new(Mutex::new(delivery_catalog())),
        }
    }

    /// The `SpawnedSession` the production spawn assembles, with the
    /// delivery pending exactly as `pending_pi_delivery` packages it. No
    /// reader thread is started here: `start_spawned_session` is what
    /// starts it, which is the fact under test.
    fn spawned_session(pi: SpawnedPi) -> SpawnedSession {
        let stdout = PiStdout::spawn(pi.stdout).expect("Pi stdout");
        let control = Arc::new(PiControl::new(
            Arc::clone(&pi.stdin),
            Arc::clone(&pi.next_id),
        ));
        let permission_broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let reader = PiReader::new(
            Vec::new(),
            SessionEvent::SessionManifest {
                provider_id: Some("pi".to_string()),
                current_model_id: None,
                models: Vec::new(),
                modes: None,
            },
            Arc::clone(&permission_broker),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::clone(&pi.next_id),
            Arc::clone(&control),
            Arc::clone(&pi.stdin),
            Arc::new(AtomicBool::new(true)),
        );
        let switcher = PiSwitcher {
            control,
            catalog: Arc::clone(&pi.catalog),
            mode_id: Arc::new(Mutex::new("ask".to_string())),
            permission_extension_active: Arc::new(AtomicBool::new(true)),
        };
        let pending_delivery = pending_pi_delivery(&switcher, &pi_delivery());
        SpawnedSession {
            process_job: JobObject::new().expect("process job"),
            master: None,
            killer: Box::new(PiKiller {
                process: Arc::clone(&pi.process),
                stdin: Arc::clone(&pi.stdin),
                next_id: Arc::clone(&pi.next_id),
                permission_broker: Arc::clone(&permission_broker),
                cancelled: Arc::new(AtomicBool::new(false)),
                extension_path: crate::test_dirs::test_temp_dir("devboule-pi-lifecycle-ext")
                    .join("extension.ts"),
                bridge_path: None,
            }),
            switcher: Some(Box::new(switcher)),
            child: Box::new(StdioWaitableChild {
                process: Arc::clone(&pi.process),
            }),
            writer: Arc::new(Mutex::new(Box::new(PiWriter {
                stdin: Arc::clone(&pi.stdin),
                next_id: Arc::clone(&pi.next_id),
                pending: Vec::new(),
            }) as Box<dyn std::io::Write + Send>)),
            image_sink: None,
            static_image_sink: Some(Arc::new(PiStaticPrompt::new(
                Arc::clone(&pi.stdin),
                Arc::clone(&pi.next_id),
                Arc::clone(&pi.catalog),
            ))),
            reader: Box::new(stdout),
            reader_dispatch: Some(Box::new(reader)),
            stderr: Some(Box::new(
                PiStderr::start(pi.stderr).expect("stderr wrapper"),
            )),
            permission_broker: Some(Arc::clone(&permission_broker)),
            os_handle: None,
            peer_session_id: None,
            agent_version: None,
            pending_delivery,
            pending_codex_verify: None,
            out_of_band: None,
        }
    }

    fn metadata(id: &str) -> devboule_protocol::Session {
        devboule_protocol::Session {
            id: id.to_string(),
            workspace_id: None,
            cwd: None,
            kind: crate::session::SessionKind::Pi,
            title: "Pi".to_string(),
            state: devboule_protocol::SessionState::Live { generation: 1 },
            elapsed_ms: Some(0),
            provider: Some("pi".to_string()),
            peer_session_id: None,
            created_at_ms: 1,
            origin: devboule_protocol::SessionOrigin::local(),
            display_name: None,
            created_by: None,
            profile_id: None,
            context_id: None,
            unattended: devboule_protocol::UnattendedState::No,
            labels: Default::default(),
            resumable: false,
        }
    }

    fn log_path(tag: &str) -> std::path::PathBuf {
        let dir = crate::test_dirs::test_temp_dir(&format!("devboule-pi-lifecycle-{tag}"));
        dir.join("commands.log")
    }

    fn read_log(log: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The argv the fake child wrote for itself: one JSON line per launch.
    fn read_argv(path: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .next()
            .and_then(|line| serde_json::from_str(line).ok())
            .unwrap_or_default()
    }

    fn wait_for_commands(log: &std::path::Path, wanted: &[&str]) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let commands = read_log(log);
            if wanted
                .iter()
                .all(|want| commands.iter().any(|command| command == want))
            {
                return commands;
            }
            assert!(
                Instant::now() < deadline,
                "the switch never reached the child: {commands:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Production never spawns a bare program name: the catalog resolves
    /// the provider executable to an absolute path before the launch
    /// (`InstalledAgent::acp_command` — element 0 is the path
    /// CreateProcess will run). The test resolves the same way, both to
    /// stay faithful to that contract and because a `current_dir` on the
    /// command changes how a bare name would be searched.
    fn node_program() -> String {
        let path = std::env::var_os("PATH").unwrap_or_default();
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("node.exe");
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
        let error = std::io::Error::new(std::io::ErrorKind::NotFound, "node.exe not on PATH");
        panic!(
            "{}",
            super::node_unavailable("Pi spawn wiring test", &error)
        );
    }

    fn child_exited(process: &Arc<Mutex<Child>>) -> bool {
        for _ in 0..100 {
            if let Ok(mut child) = process.lock() {
                if child.try_wait().ok().flatten().is_some() {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// The answer to F1: the delivery completes because the session
    /// reader — the thread `start_spawned_session` starts, and nothing
    /// else — delivers the switch's answer. The three requests the
    /// switcher writes are on the wire and answered by the time the
    /// start returns.
    #[test]
    fn the_session_reader_delivers_the_profile_switch_start_spawned_session_awaits_it() {
        let log = log_path("ok");
        let state = ServerState::new("pi-delivery-lifecycle".to_string());
        let owner = OwnerId::new("local", "test").expect("owner");
        let pi = spawned_pi(super::FAKE_PI_DELIVERY_ANSWERS, &log);
        let session_id = "pifelifecycleok1".to_string();

        start_spawned_session(
            &state,
            &state.sessions,
            metadata(&session_id),
            owner.clone(),
            None,
            Some("ask".to_string()),
            spawned_session(pi),
            None,
        )
        .expect("the delivery completes once the session reader is live");

        let commands = wait_for_commands(
            &log,
            &[
                "set_model",
                "get_available_thinking_levels",
                "set_thinking_level",
            ],
        );
        assert!(
            commands.contains(&"set_model".to_string()),
            "the switch is on the wire: {commands:?}"
        );
        let _ = state.sessions.close(&session_id, &owner, &None);
        let _ = std::fs::remove_dir_all(log.parent().expect("log dir"));
    }

    /// The other half of the repair: a refused switch is still a
    /// refusal — the hook's error tears the child down and fails the
    /// session start, so no child the card did not describe survives to
    /// answer anything, and the roster never lists it.
    #[test]
    fn a_refused_delivery_tears_the_child_down_and_fails_the_start() {
        let log = log_path("refused");
        let state = ServerState::new("pi-delivery-refusal".to_string());
        let owner = OwnerId::new("local", "test").expect("owner");
        let pi = spawned_pi(super::FAKE_PI_DELIVERY_REFUSES, &log);
        let process = Arc::clone(&pi.process);
        let session_id = "pifelifecycleref1".to_string();

        let error = start_spawned_session(
            &state,
            &state.sessions,
            metadata(&session_id),
            owner.clone(),
            None,
            Some("ask".to_string()),
            spawned_session(pi),
            None,
        )
        .expect_err("a refused switch must refuse the session start");

        assert!(
            error.message.contains("set_model failed"),
            "the refusal names the refused rpc: {}",
            error.message
        );
        assert!(
            !state
                .sessions
                .list(&owner)
                .expect("roster")
                .iter()
                .any(|session| session.id == session_id),
            "the refused child is not left on the roster"
        );
        assert!(
            child_exited(&process),
            "the child was killed by the refusal teardown"
        );
        let _ = std::fs::remove_dir_all(log.parent().expect("log dir"));
    }

    /// The re-audit's P2-1: the session is not visible until it is
    /// configured. The gated fake holds the `set_model` answer, so the
    /// delivery — and with it the whole create — sits in flight while
    /// the child is already spawned and the reader already running. In
    /// that window the registry holds the entry as `Configuring`: no
    /// roster read may hand the id out, because a prompt sent now would
    /// be silently discarded if the delivery were refused. The old
    /// insert-as-live shape fails the not-listed assertion here.
    #[test]
    fn a_session_is_not_listed_until_its_delivery_lands() {
        let log = log_path("window");
        let gate = log.parent().expect("log dir").join("gate.txt");
        let _ = std::fs::remove_file(&gate);
        let state = ServerState::new("pi-delivery-window".to_string());
        let owner = OwnerId::new("local", "test").expect("owner");
        let pi = spawned_pi_with_env(
            super::FAKE_PI_DELIVERY_GATED,
            &log,
            &[("DEVBOULE_FAKE_PI_GATE", gate.to_string_lossy().into_owned())],
        );
        let session_id = "pifelifecyclewin1".to_string();
        let metadata = metadata(&session_id);
        let owner_for_start = owner.clone();
        let state_for_start = Arc::clone(&state);
        let start = std::thread::Builder::new()
            .name("pi-delivery-window-start".to_string())
            .spawn(move || {
                start_spawned_session(
                    &state_for_start,
                    &state_for_start.sessions,
                    metadata,
                    owner_for_start,
                    None,
                    Some("ask".to_string()),
                    spawned_session(pi),
                    None,
                )
                .expect("the delivery completes once the gate opens")
            })
            .expect("spawn the create thread");

        // The request is on the wire and the fake is holding its answer:
        // the delivery — and therefore the create — is in flight now.
        wait_for_commands(&log, &["set_model"]);
        for _ in 0..10 {
            assert!(
                !state
                    .sessions
                    .list(&owner)
                    .expect("roster")
                    .iter()
                    .any(|session| session.id == session_id),
                "a session inside its delivery window is not listed"
            );
            assert!(
                !state
                    .sessions
                    .state_snapshots(&owner)
                    .iter()
                    .any(|snapshot| snapshot.id == session_id),
                "a session inside its delivery window has no snapshot"
            );
            // The repair pass's P1-1, reconciled with the one-door
            // design: the delete here is refused `SessionNotFound` —
            // the answer `peer_entry` gives every id-addressed peer
            // call for a `Configuring` entry — not the close-first
            // refusal. The decision, so the next reader does not
            // relitigate it:
            // - it is what the variant's own contract says — a
            //   `Configuring` entry is invisible to every id-addressed
            //   peer call;
            // - it does not confirm to a peer that the id exists, and
            //   during the window no peer legitimately holds that id
            //   (the create returns it only after promotion);
            // - an API where `sessions_list` says the session does not
            //   exist while `delete` says "close it first" contradicts
            //   itself. One door, one answer.
            // What the refusal must still prevent is the P1's harm: the
            // windowed entry holds a running child, and an unrefused
            // delete here would remove that child's entry mid-delivery.
            // Non-removal is proved end to end below: the gate opens,
            // the create completes, the id is listed.
            let delete_error = state
                .sessions
                .delete_session(&session_id, &owner)
                .expect_err("delete inside the delivery window must be refused");
            assert_eq!(
                    delete_error.code,
                    devboule_protocol::ErrorCode::SessionNotFound,
                    "the windowed delete takes the peer-visibility door's answer, not the close-first guard: {delete_error:?}"
                );
            std::thread::sleep(Duration::from_millis(50));
        }

        // The gate opens, the delivery lands, the create returns — and
        // only then does the session exist for its peers. Resume's guard
        // (the re-audit's P2-1) sits behind the journal-row lookup and
        // `resume_handle`, and a resumable row takes that path — so the
        // resume refusal is observed by writing the row a resumed ACP
        // child carries and naming the windowed id the way the audit's
        // trigger describes. The named-provider resolution the resume
        // performs before the guard needs the direct-command override.
        let _acp_env = crate::session::lock_acp_env();
        std::env::set_var("DEVBOULE_ACP_COMMAND", r#"["cmd"]"#);
        std::env::set_var("DEVBOULE_ACP_PROVIDER_ID", "devboule-acp-stub");
        if let Some(journal) = state.sessions.journal.as_ref() {
            let mut row = crate::journal::new_session_record(
                session_id.clone(),
                owner.user.clone(),
                None,
                devboule_protocol::SessionKind::Acp,
                "gated delivery",
            );
            row.provider = Some("devboule-acp-stub".to_string());
            row.peer_session_id = Some("stub-session".to_string());
            journal
                .upsert_blocking(row)
                .expect("the resumed row is on the journal");
        }
        let conn = crate::session::ConnHandle::new(91);
        let resume_error = state
            .sessions
            .resume(&state, &session_id, &owner, &conn)
            .expect_err("resume inside the delivery window must be refused");
        assert!(
            resume_error
                .message
                .contains("cannot be resumed while its process is running"),
            "the resume refusal names the running child: {resume_error:?}"
        );
        std::env::remove_var("DEVBOULE_ACP_COMMAND");
        std::env::remove_var("DEVBOULE_ACP_PROVIDER_ID");
        // Still refused, still present: the guard is a refusal, not a
        // teardown, and the child the create will return is untouched.
        assert!(
            !state
                .sessions
                .list(&owner)
                .expect("roster")
                .iter()
                .any(|session| session.id == session_id),
            "the windowed child stays hidden after the refused resume"
        );

        std::fs::write(&gate, b"go").expect("open the gate");
        start.join().expect("the create thread");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let listed = state
                .sessions
                .list(&owner)
                .expect("roster")
                .iter()
                .any(|session| session.id == session_id);
            assert!(
                listed || Instant::now() < deadline,
                "the session is listed once its delivery has landed"
            );
            if listed {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // The close-first arm must not go silently dead above the
        // window: the same id, promoted, is a peer-visible session
        // holding a running child, and its delete is refused with the
        // close-first refusal — the arm the `SessionNotFound`
        // reconciliation answers around, not removes. A suite that lost
        // this assertion would delete the guard by unreachability.
        let close_first_error = state
            .sessions
            .delete_session(&session_id, &owner)
            .expect_err("deleting a live session must be refused");
        assert_eq!(
            close_first_error.code,
            devboule_protocol::ErrorCode::InvalidRequest,
            "the live delete is the close-first refusal: {close_first_error:?}"
        );
        assert_eq!(
            close_first_error.message, "Close the session before deleting it.",
            "the live delete refusal demands the close"
        );
        assert!(
            state
                .sessions
                .list(&owner)
                .expect("roster")
                .iter()
                .any(|session| session.id == session_id),
            "the refused live delete leaves the session listed"
        );
        let _ = state.sessions.close(&session_id, &owner, &None);
        let _ = std::fs::remove_dir_all(log.parent().expect("log dir"));
    }

    /// The re-audit's P2-3: the seam the F1 repair created is
    /// `spawn_process` building the hook and handing it to the
    /// `SpawnedSession` — and no test called `spawn_process` for pi at
    /// all, so passing `None` there left the suite green while the
    /// profile's model died silently at spawn. This test runs the real
    /// seam: the real `spawn_process` (its handshake served by the fake,
    /// its extension written into the state's runtime dir), its output
    /// fed to the real `start_spawned_session`, and the profile's switch
    /// asserted on the child's wire. Cut the wiring — `None` in place of
    /// the hook — and nothing ever answers the switch: the wait times
    /// out, red.
    #[test]
    fn spawn_process_wires_the_delivery_the_reader_will_run() {
        let log = log_path("spawnwiring");
        let state = ServerState::new("pi-spawn-wiring".to_string());
        let owner = OwnerId::new("local", "test").expect("owner");
        // The fake is a script FILE the way a real Pi launch carries its
        // entry, with `--` ending the node options: `spawn_args` injects
        // `--mode rpc` and the extension path after the entry, and those
        // are the entry's argv, not node's.
        let script = log.parent().expect("log dir").join("fake-pi-entry.js");
        std::fs::write(&script, super::FAKE_PI_SPAWN_HANDSHAKE).expect("write the fake pi entry");
        let command = PtyCommand::new(
            node_program(),
            vec![script.to_string_lossy().into_owned(), "--".to_string()],
            crate::test_dirs::test_temp_dir("devboule-pi-cwd"),
            vec![(
                "DEVBOULE_FAKE_PI_LOG".to_string(),
                log.to_string_lossy().into_owned(),
            )],
        );
        let delivery =
            ProfileDelivery::for_child("bypass", "pi-model", Some("low"), &serde_json::Map::new());
        let spawned = spawn_process(&state, command, None, delivery)
            .expect("spawn_process assembles the child and the delivery hook");
        let session_id = "pifelifecyclespawn1".to_string();
        start_spawned_session(
            &state,
            &state.sessions,
            metadata(&session_id),
            owner.clone(),
            None,
            None,
            spawned,
            None,
        )
        .expect("the delivery lands once the session reader is live");
        let commands = wait_for_commands(
            &log,
            &[
                "set_model",
                "get_available_thinking_levels",
                "set_thinking_level",
            ],
        );
        assert!(
            commands.contains(&"set_model".to_string()),
            "the profile's switch reached the child through production's own wiring: {commands:?}"
        );
        let _ = state.sessions.close(&session_id, &owner, &None);
        let _ = std::fs::remove_dir_all(log.parent().expect("log dir"));
    }

    /// The handle's wiring: `spawn_process_resuming` puts the persisted
    /// session id on the child's own launch line as `--session <id>` — the
    /// one place the child can read it — and the fresh road, spawned with
    /// the same fake as a control, carries no session flag at all. The child
    /// writes its argv itself, so the assertion is about what the process
    /// received, not about what the caller intended. Cut the splice and the
    /// resumed child comes up on a new conversation: the first assertion is
    /// the one that goes red.
    #[test]
    fn spawn_process_resuming_puts_the_handle_on_the_childs_argv() {
        const HANDLE: &str = "01a0c1a7-0b95-731a-9a2e-06db94ff8043";
        let log = log_path("resumeargv");
        let state = ServerState::new("pi-resume-argv".to_string());
        let dir = log.parent().expect("log dir").to_path_buf();
        let script = dir.join("fake-pi-entry.js");
        std::fs::write(&script, super::FAKE_PI_RESUME_HANDSHAKE).expect("write the fake pi entry");
        let base = PtyCommand::new(
            node_program(),
            vec![script.to_string_lossy().into_owned(), "--".to_string()],
            crate::test_dirs::test_temp_dir("devboule-pi-cwd"),
            vec![
                (
                    "DEVBOULE_FAKE_PI_LOG".to_string(),
                    log.to_string_lossy().into_owned(),
                ),
                ("DEVBOULE_FAKE_PI_SESSION".to_string(), HANDLE.to_string()),
            ],
        );

        let resumed_argv = dir.join("resumed-argv.log");
        let mut resumed = base.clone();
        resumed.env.push((
            "DEVBOULE_FAKE_PI_ARGV_LOG".to_string(),
            resumed_argv.to_string_lossy().into_owned(),
        ));
        let resumed_session = spawn_process_resuming(&state, resumed, HANDLE.to_string(), None)
            .expect("the resumed launch completes its handshake");
        let argv = read_argv(&resumed_argv);
        assert_eq!(
            argv.iter().filter(|arg| *arg == "--session").count(),
            1,
            "the child's argv carries the session option exactly once: {argv:?}"
        );
        assert!(
            argv.windows(2).any(|pair| pair == ["--session", HANDLE]),
            "the child's argv carries the persisted handle verbatim: {argv:?}"
        );
        for flag in [
            "--resume",
            "-r",
            "--continue",
            "-c",
            "--session-id",
            "--fork",
            "--no-session",
        ] {
            assert!(
                !argv.iter().any(|arg| arg == flag),
                "the resume road selects the conversation one way only ({flag}): {argv:?}"
            );
        }
        assert!(
            argv.windows(2)
                .any(|pair| pair[0] == "-e" && pair[1].ends_with(".ts")),
            "the permission extension is still on the resumed launch line: {argv:?}"
        );
        // Windows' kill-on-job-close ends the fake when the session drops;
        // elsewhere the dropped stdin ends it. No reader is started here.
        drop(resumed_session);

        let fresh_argv = dir.join("fresh-argv.log");
        let mut fresh = base;
        fresh.env.push((
            "DEVBOULE_FAKE_PI_ARGV_LOG".to_string(),
            fresh_argv.to_string_lossy().into_owned(),
        ));
        let fresh_session = spawn_process(&state, fresh, None, ProfileDelivery::none())
            .expect("the fresh launch completes its handshake");
        let fresh_args = read_argv(&fresh_argv);
        assert!(
            !fresh_args.iter().any(|arg| arg == "--session"),
            "the fresh road must not carry a session flag: {fresh_args:?}"
        );
        drop(fresh_session);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
