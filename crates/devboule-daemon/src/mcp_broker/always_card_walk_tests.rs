//! The two acts that always ask, found by tracing where a call can land rather
//! than by naming tools: every tool whose schema can name a paired device, and
//! every source file that dials one, is classified in `always_card::PEER_ROADS`
//! — a new road to another machine is a red test until someone decides it.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::dispatch::enabled_tool_list;
use super::tools::always_card::{always_card, AlwaysCard, Effect, PEER_ROADS};
use crate::provider_catalog::{
    ToolOverlay, MCP_BROKER_TOOLS, MCP_BROWSER_FILL_LOGIN_TOOL, MCP_LIST_PEER_AGENTS_TOOL,
    MCP_SEND_MESSAGE_TOOL,
};

/// Every tool the broker lists, with the schema an agent is offered.
fn listed_schemas() -> Vec<(String, Value)> {
    enabled_tool_list(MCP_BROKER_TOOLS, None, ToolOverlay::NONE)
        .into_iter()
        .map(|tool| {
            let name = tool["name"].as_str().expect("a tool name").to_string();
            (name, tool["inputSchema"].clone())
        })
        .collect()
}

#[test]
fn every_tool_that_can_name_a_paired_device_is_classified() {
    let naming: BTreeSet<String> = listed_schemas()
        .into_iter()
        .filter(|(_, schema)| schema.pointer("/properties/deviceId").is_some())
        .map(|(name, _)| name)
        .collect();
    let classified: BTreeSet<String> = PEER_ROADS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();

    assert!(
        !naming.is_empty(),
        "the walk found the tools that name a device"
    );
    assert_eq!(
        naming, classified,
        "a tool whose schema names a device needs a row in PEER_ROADS, and no row may outlive its tool"
    );
}

#[test]
fn every_tool_that_acts_on_a_paired_device_goes_through_the_always_card_predicate() {
    for (tool, effect) in PEER_ROADS {
        let aimed = json!({"deviceId": "dev-far", "to_agent": "s.far", "text": "hi"});
        match effect {
            Effect::Acts { .. } => assert!(
                matches!(always_card(tool, &aimed), Some(AlwaysCard::PairedDevice(_))),
                "{tool} acts on a paired device and must always ask"
            ),
            Effect::Reads => assert!(
                always_card(tool, &aimed).is_none(),
                "{tool} only reads the device and asks nobody"
            ),
        }
        // Without a device the call is local and follows the mode.
        assert!(
            always_card(tool, &json!({"to_agent": "s.local", "text": "hi"})).is_none(),
            "{tool} without a deviceId is the local road"
        );
    }
}

/// The only two acts that always ask: nothing else is, with a device named or
/// not, so no third exception (and no card for a push to main, which no tool
/// does) can arrive unnoticed.
#[test]
fn main_push_has_no_new_card_and_nothing_else_always_asks() {
    let mut asking = BTreeSet::new();
    for (name, _) in MCP_BROKER_TOOLS {
        for arguments in [
            json!({}),
            json!({"deviceId": "dev-far", "to_agent": "x", "text": "y"}),
        ] {
            if always_card(name, &arguments).is_some() {
                asking.insert(*name);
            }
        }
    }
    assert_eq!(
        asking,
        BTreeSet::from([MCP_SEND_MESSAGE_TOOL, MCP_BROWSER_FILL_LOGIN_TOOL]),
        "a command to another machine and a saved login — and nothing else"
    );
    for (name, _) in MCP_BROKER_TOOLS {
        assert!(
            !["push", "land", "merge"]
                .iter()
                .any(|word| name.contains(word)),
            "{name}: a tool that pushes to main would need the owner's decision first; \
             the owner decided it gets no card"
        );
    }
}

/// The roster read dials the device without sending it anything: it stays
/// unasked, and the walk above knows why.
#[test]
fn the_peer_roster_read_is_the_one_classified_read() {
    let reads: Vec<&str> = PEER_ROADS
        .iter()
        .filter(|(_, effect)| matches!(effect, Effect::Reads))
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(reads, [MCP_LIST_PEER_AGENTS_TOOL]);
}

fn source_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("source directory") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            source_files(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// Every file that dials a paired device is one of the two roads classified
/// above; a third one is a road to another machine nobody decided.
#[test]
fn every_source_file_that_dials_a_paired_device_is_a_classified_road() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    source_files(&src.join("mcp_broker"), &mut files);
    files.push(src.join("mcp_peer_agents.rs"));

    let dialling: BTreeSet<String> = files
        .iter()
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            !name.ends_with("_tests.rs") && name != "tests.rs"
        })
        .filter(|path| {
            fs::read_to_string(path)
                .expect("a source file")
                .lines()
                .any(|line| {
                    !line.trim_start().starts_with("//")
                        && (line.contains("call_peer(") || line.contains("dial_peer("))
                })
        })
        .map(|path| {
            path.strip_prefix(&src)
                .expect("under src")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();

    assert_eq!(
        dialling,
        BTreeSet::from([
            "mcp_broker/tools/messaging_peer.rs".to_string(),
            "mcp_peer_agents.rs".to_string(),
        ]),
        "messaging_peer.rs is the send (carded), mcp_peer_agents.rs the roster read"
    );
}

/// Agent-originated dials carry agent frames, never session frames: the two
/// classified roads above may send `AgentMessageSend` (the carded send) and
/// `PeerAgentsList` (the roster read) and nothing else. Session frames —
/// creates, sends, resizes, closes, modes — travel only over the held peer
/// link, which only queues what the app door admitted as human-originated.
/// A tool that dialled a session frame would reach the far daemon's human
/// scope wearing an agent's intent, so the frame set is pinned here.
#[test]
fn agent_dials_carry_no_session_frames() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let allowed = ["AgentMessageSend", "PeerAgentsList", "Hello", "Ping"];
    for file in ["mcp_broker/tools/messaging_peer.rs", "mcp_peer_agents.rs"] {
        let text = fs::read_to_string(src.join(file)).expect("a classified dialling source");
        let mut constructed = BTreeSet::new();
        for line in text.lines() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            // `ClientMessage::Variant` constructions and matches alike: a
            // match arm that names a session frame is a road that handles
            // one, which is what this walk forbids.
            let mut rest = code;
            while let Some(at) = rest.find("ClientMessage::") {
                rest = &rest[at + "ClientMessage::".len()..];
                let end = rest
                    .find(|character: char| !character.is_alphanumeric() && character != '_')
                    .unwrap_or(rest.len());
                constructed.insert(rest[..end].to_string());
            }
        }
        assert!(
            !constructed.is_empty(),
            "{file} must dial something, or the walk proves nothing"
        );
        let forbidden: Vec<_> = constructed
            .into_iter()
            .filter(|variant| !allowed.contains(&variant.as_str()))
            .collect();
        assert!(
            forbidden.is_empty(),
            "{file} dials session frames an agent must never send: {forbidden:?}"
        );
    }
}
