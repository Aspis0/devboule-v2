use std::fmt;

use crate::login_shell_env::LoginShellCaptureOutcome;
use devboule_protocol::JournalStats;
#[cfg(feature = "server")]
use devboule_protocol::{ProviderInfo, Session, SessionState};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A string that has crossed the diagnostics redaction boundary. The raw
/// value is private so a report cannot accidentally serialize a pre-redaction
/// value. Construction applies the same secret-token classes as
/// `oracle_core::redact_secret_tokens`; client-side deserialization rechecks
/// identifiers as well. The agreement corpus in `src-tauri` keeps this local
/// copy in sync without making the daemon's client-only build depend on the
/// heavier oracle-core crate.
#[derive(Clone, PartialEq, Eq)]
pub struct SafeText(String);

impl SafeText {
    pub fn new(raw: impl AsRef<str>) -> Self {
        Self(redact_identifiers(raw.as_ref()))
    }

    fn from_wire(raw: String) -> Self {
        Self(redact_identifiers(&raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SafeText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("SafeText").field(&self.0).finish()
    }
}

impl Serialize for SafeText {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SafeText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::from_wire)
    }
}

fn redact_identifiers(value: &str) -> String {
    redact_sids(&redact_pipe_names(&redact_windows_home_paths(
        &redact_secret_tokens(value),
    )))
}

/// Keep the diagnostics boundary dependency-free: the daemon is also built as
/// a client-only library for Tauri. This mirrors
/// `oracle_core::redact_secret_tokens` for known provider tokens, assignments,
/// bearer/JWT values, high-entropy runs, and long hex runs, then the enclosing
/// diagnostics redactor adds Windows home paths and SIDs. The Tauri agreement
/// test checks containment: this copy is intentionally the superset, while
/// the two redactors remain separate to avoid a heavy oracle-core dependency
/// in the daemon's client-only build.
fn redact_secret_tokens(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut spans = Vec::new();
    let keys = [
        "api_key",
        "api-key",
        "secret",
        "token",
        "password",
        "passwd",
        "access_key",
        "access-key",
    ];
    for key in keys {
        let lower = value.to_ascii_lowercase();
        let mut search_from = 0;
        while let Some(relative) = lower[search_from..].find(key) {
            let start = search_from + relative;
            let end_key = start + key.len();
            let boundary_before = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
            let boundary_after = end_key == bytes.len() || !bytes[end_key].is_ascii_alphanumeric();
            if boundary_before && boundary_after {
                let mut value_start = end_key;
                while value
                    .as_bytes()
                    .get(value_start)
                    .is_some_and(u8::is_ascii_whitespace)
                {
                    value_start += 1;
                }
                if matches!(value.as_bytes().get(value_start), Some(b'=' | b':')) {
                    value_start += 1;
                    while value
                        .as_bytes()
                        .get(value_start)
                        .is_some_and(u8::is_ascii_whitespace)
                    {
                        value_start += 1;
                    }
                    let quoted = matches!(value.as_bytes().get(value_start), Some(b'\'' | b'"'));
                    if quoted {
                        value_start += 1;
                    }
                    let mut value_end = value_start;
                    while value_end < bytes.len()
                        && if quoted {
                            !matches!(bytes[value_end], b'\'' | b'"')
                        } else {
                            !bytes[value_end].is_ascii_whitespace()
                        }
                    {
                        value_end += 1;
                    }
                    if value_end > value_start {
                        spans.push((value_start, value_end));
                    }
                }
            }
            search_from = end_key;
        }
    }

    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        while index < bytes.len()
            && (bytes[index].is_ascii_alphanumeric()
                || matches!(bytes[index], b'_' | b'-' | b'.' | b'/' | b'+' | b'='))
        {
            index += 1;
        }
        if index == start {
            index += 1;
            continue;
        }
        let token = &value[start..index];
        let lower = token.to_ascii_lowercase();
        let known_prefix = lower.starts_with("ghp_")
            || lower.starts_with("gho_")
            || lower.starts_with("ghs_")
            || lower.starts_with("ghu_")
            || lower.starts_with("ghr_")
            || lower.starts_with("github_pat_")
            || lower.starts_with("scw")
            || lower.starts_with("akia")
            || lower.starts_with("xoxb-")
            || lower.starts_with("xoxa-")
            || lower.starts_with("xoxp-")
            || lower.starts_with("xoxs-")
            || lower.starts_with("xopr-")
            || lower.starts_with("xoxr-")
            || lower.starts_with("eyj") && token.matches('.').count() >= 2;
        let long_hex = token.len() >= 40 && token.bytes().all(|byte| byte.is_ascii_hexdigit());
        let mixed_entropy = token.len() >= 40
            && token
                .chars()
                .any(|character| character.is_ascii_lowercase())
            && token
                .chars()
                .any(|character| character.is_ascii_uppercase())
            && token.chars().any(|character| character.is_ascii_digit());
        if known_prefix || long_hex || mixed_entropy {
            spans.push((start, index));
        }
    }

    let lower = value.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(relative) = lower[search_from..].find("bearer ") {
        let start = search_from + relative + "bearer ".len();
        let mut end = start;
        while end < bytes.len() && !bytes[end].is_ascii_whitespace() {
            end += 1;
        }
        if end > start {
            spans.push((start, end));
        }
        search_from = end.max(start + 1);
    }

    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in spans {
        if let Some((_, previous_end)) = merged.last_mut() {
            if start <= *previous_end {
                *previous_end = (*previous_end).max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    let mut redacted = value.to_string();
    for (start, end) in merged.into_iter().rev() {
        let newlines = redacted[start..end]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        let mut replacement = String::from("[redacted-secret]");
        replacement.extend(std::iter::repeat_n('\n', newlines));
        redacted.replace_range(start..end, &replacement);
    }
    redacted
}

/// Replace the user component of home paths, including paths supplied by the
/// runtime directory and provider diagnostics. The daemon currently receives
/// Windows runtime directories, but Unix home forms are handled now so a
/// macOS/Linux port cannot silently violate the diagnostics redaction promise.
/// This is deliberately local to the report because oracle-core redacts
/// secrets, not identities.
fn redact_windows_home_paths(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        // The daemon currently gets runtime directories from LOCALAPPDATA, so
        // reports do not produce UNC paths today. Redirected or roaming
        // profiles can supply one, though, and it must cross the same boundary
        // before diagnostics leave the process.
        let is_unc = index + 1 < bytes.len()
            && matches!(bytes[index], b'\\' | b'/')
            && matches!(bytes[index + 1], b'\\' | b'/');
        if is_unc {
            let server_start = index + 2;
            if let Some(server_end) = bytes[server_start..]
                .iter()
                .position(|byte| matches!(byte, b'\\' | b'/'))
                .map(|offset| server_start + offset)
            {
                let share_start = server_end + 1;
                if let Some(share_end) = bytes[share_start..]
                    .iter()
                    .position(|byte| matches!(byte, b'\\' | b'/'))
                    .map(|offset| share_start + offset)
                {
                    let users_start = share_end + 1;
                    if let Some(users_end) = bytes[users_start..]
                        .iter()
                        .position(|byte| matches!(byte, b'\\' | b'/'))
                        .map(|offset| users_start + offset)
                    {
                        if value[users_start..users_end].eq_ignore_ascii_case("Users") {
                            let user_start = users_end + 1;
                            let user_end = bytes[user_start..]
                                .iter()
                                .position(|byte| matches!(byte, b'\\' | b'/'))
                                .map(|offset| user_start + offset)
                                .unwrap_or(bytes.len());
                            output.push_str("[redacted-home]");
                            index = user_end;
                            continue;
                        }
                    }
                }
            }
        }

        if let Some(user_end) = unix_home_user_end(value, bytes, index) {
            output.push_str("[redacted-home]");
            index = user_end;
            continue;
        }

        let is_drive = index + 3 <= bytes.len()
            && bytes[index].is_ascii_alphabetic()
            && bytes[index + 1] == b':'
            && matches!(bytes[index + 2], b'\\' | b'/');
        if !is_drive {
            let character = value[index..].chars().next().expect("valid utf-8 boundary");
            output.push(character);
            index += character.len_utf8();
            continue;
        }
        let component_start = index + 3;
        let Some(component_end) = bytes[component_start..]
            .iter()
            .position(|byte| matches!(byte, b'\\' | b'/'))
            .map(|offset| component_start + offset)
        else {
            output.push_str(&value[index..]);
            break;
        };
        if !value[component_start..component_end].eq_ignore_ascii_case("Users") {
            let character = value[index..].chars().next().expect("valid utf-8 boundary");
            output.push(character);
            index += character.len_utf8();
            continue;
        }
        let user_start = component_end + 1;
        let user_end = bytes[user_start..]
            .iter()
            .position(|byte| matches!(byte, b'\\' | b'/'))
            .map(|offset| user_start + offset)
            .unwrap_or(bytes.len());
        output.push_str("[redacted-home]");
        index = user_end;
    }
    output
}

fn unix_home_user_end(value: &str, bytes: &[u8], index: usize) -> Option<usize> {
    if bytes.get(index) != Some(&b'/')
        || (index > 0
            && !matches!(
                bytes[index - 1],
                b' ' | b'\t' | b'\r' | b'\n' | b'(' | b'[' | b'{' | b'=' | b':' | b','
            ))
    {
        return None;
    }

    let prefix = if value[index..].starts_with("/Users/") {
        "/Users/"
    } else if value[index..].starts_with("/home/") {
        "/home/"
    } else {
        return None;
    };
    let user_start = index + prefix.len();
    let user_end = bytes[user_start..]
        .iter()
        .position(|byte| matches!(byte, b'/' | b' ' | b'\t' | b'\r' | b'\n'))
        .map(|offset| user_start + offset)
        .unwrap_or(bytes.len());
    (user_end > user_start).then_some(user_end)
}

/// Replace the deterministic pipe discriminator. It is deliberately not
/// truncated: even a shortened FNV-1a value is useful against a small
/// username wordlist, while the exact hash has no diagnostic value.
fn redact_pipe_names(value: &str) -> String {
    const PREFIX: &str = r"\\.\pipe\devboule-";
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < value.len() {
        let Some(relative_start) = value[cursor..].find(PREFIX) else {
            output.push_str(&value[cursor..]);
            break;
        };
        let start = cursor + relative_start;
        let hash_start = start + PREFIX.len();
        let hash_end = hash_start + 16;
        output.push_str(&value[cursor..start]);
        if hash_end <= bytes.len()
            && bytes[hash_start..hash_end]
                .iter()
                .all(|byte| byte.is_ascii_hexdigit())
            && bytes
                .get(hash_end)
                .is_none_or(|byte| !byte.is_ascii_hexdigit())
        {
            output.push_str(PREFIX);
            output.push_str("[redacted]");
            cursor = hash_end;
        } else {
            output.push_str(PREFIX);
            cursor = hash_start;
        }
    }
    output
}

/// Replace Windows SID-shaped identifiers. The peer SID is intentionally not
/// collected into a report at all; this second boundary protects free-text
/// error/authentication reasons that happen to contain one.
fn redact_sids(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        let starts_sid = (bytes[index] == b'S' || bytes[index] == b's')
            && index + 2 < bytes.len()
            && bytes[index + 1] == b'-'
            && bytes[index + 2].is_ascii_digit()
            && (index == 0 || !bytes[index - 1].is_ascii_alphanumeric());
        if !starts_sid {
            let character = value[index..].chars().next().expect("valid utf-8 boundary");
            output.push(character);
            index += character.len_utf8();
            continue;
        }
        let mut end = index + 2;
        let mut hyphens = 0;
        while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'-') {
            hyphens += usize::from(bytes[end] == b'-');
            end += 1;
        }
        if hyphens < 2 || (end < bytes.len() && bytes[end].is_ascii_alphanumeric()) {
            let character = value[index..].chars().next().expect("valid utf-8 boundary");
            output.push(character);
            index += character.len_utf8();
            continue;
        }
        output.push_str("[redacted-sid]");
        index = end;
    }
    output
}

#[cfg(feature = "server")]
#[derive(Clone, Debug)]
pub struct DiagnosticsInput {
    pub instance_id: String,
    pub daemon_version: String,
    pub protocol_version: u32,
    pub pid: u32,
    pub uptime_ms: u64,
    pub clients: u32,
    pub daemon_sessions: u32,
    pub capabilities: Vec<String>,
    pub peak_ring_bytes: u64,
    pub ring_evicted_bytes: u64,
    pub ring_dropped_frames: u64,
    pub journal_stats: Option<JournalStats>,
    pub journal_error: Option<String>,
    pub journal_schema_version: i32,
    pub journal_file_bytes: Option<u64>,
    pub sessions: Vec<Session>,
    pub providers: Vec<ProviderInfo>,
    pub os_version: String,
    pub app_version: String,
    pub runtime_dir: String,
    pub pipe_name: String,
    pub login_shell_capture: LoginShellCaptureOutcome,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    pub daemon: DaemonDiagnostics,
    pub health: HealthDiagnostics,
    pub sessions: SessionDiagnostics,
    pub providers: Vec<ProviderDiagnostics>,
    pub environment: EnvironmentDiagnostics,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonDiagnostics {
    pub version: String,
    pub protocol_version: u32,
    pub pid: u32,
    pub uptime_ms: u64,
    pub clients: u32,
    pub sessions: u32,
    pub capabilities: Vec<String>,
    pub instance_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthDiagnostics {
    pub peak_ring_bytes: u64,
    pub ring_evicted_bytes: u64,
    pub ring_dropped_frames: u64,
    pub journal_stats: Option<JournalStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_error: Option<SafeText>,
    pub journal_schema_version: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_file_bytes: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDiagnostics {
    pub total: u64,
    pub live: u64,
    pub silent: u64,
    pub ended: u64,
    pub recovered: u64,
    pub terminal: u64,
    pub acp: u64,
    pub claude: u64,
    pub pi: u64,
    pub codex: u64,
    pub resumable: u64,
    pub oldest_live_age_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDiagnostics {
    pub id: SafeText,
    pub protocol: Option<SafeText>,
    pub origin: Option<SafeText>,
    pub install_channel: Option<SafeText>,
    pub installed_version: Option<SafeText>,
    pub latest_version: Option<SafeText>,
    pub agent_version: Option<SafeText>,
    pub installed: bool,
    pub authentication: SafeText,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDiagnostics {
    pub os_version: SafeText,
    pub app_version: String,
    pub runtime_dir: SafeText,
    pub pipe_name: SafeText,
    pub login_shell_capture: LoginShellCaptureOutcome,
}

impl DiagnosticsReport {
    /// Build the public report and cross every free-text value through the
    /// private redaction type. Session titles are deliberately not copied:
    /// they are arbitrary user text (including shell prompts with secrets).
    /// No transcript, AgentStderr, or permission request env/args/cwd is
    /// collected here. Peer SIDs are not on the wire and are never added.
    /// Raw executable paths, DEVBOULE_BIN_PATH, and other home-bearing values
    /// are likewise absent; provider rows copy metadata, never executables.
    #[cfg(feature = "server")]
    pub fn new(input: DiagnosticsInput) -> Self {
        let mut sessions = SessionDiagnostics {
            total: input.sessions.len() as u64,
            ..SessionDiagnostics::default()
        };
        for session in &input.sessions {
            match session.state {
                SessionState::Live { .. } => {
                    sessions.live += 1;
                    sessions.oldest_live_age_ms =
                        max_age(sessions.oldest_live_age_ms, session.elapsed_ms);
                }
                SessionState::Silent { .. } => {
                    sessions.silent += 1;
                    sessions.oldest_live_age_ms =
                        max_age(sessions.oldest_live_age_ms, session.elapsed_ms);
                }
                SessionState::Ended { .. } => sessions.ended += 1,
                SessionState::Recovered { .. } => sessions.recovered += 1,
            }
            match session.kind {
                devboule_protocol::SessionKind::Terminal => sessions.terminal += 1,
                devboule_protocol::SessionKind::Acp => sessions.acp += 1,
                devboule_protocol::SessionKind::Claude => sessions.claude += 1,
                devboule_protocol::SessionKind::Pi => sessions.pi += 1,
                devboule_protocol::SessionKind::Codex => sessions.codex += 1,
            }
            if !session.state.is_live()
                && matches!(session.kind, devboule_protocol::SessionKind::Acp)
                && session.provider.is_some()
                && session.peer_session_id.is_some()
            {
                sessions.resumable += 1;
            }
        }

        Self {
            daemon: DaemonDiagnostics {
                version: input.daemon_version,
                protocol_version: input.protocol_version,
                pid: input.pid,
                uptime_ms: input.uptime_ms,
                clients: input.clients,
                sessions: input.daemon_sessions,
                capabilities: input.capabilities,
                instance_id: input.instance_id,
            },
            health: HealthDiagnostics {
                peak_ring_bytes: input.peak_ring_bytes,
                ring_evicted_bytes: input.ring_evicted_bytes,
                ring_dropped_frames: input.ring_dropped_frames,
                journal_stats: input.journal_stats,
                journal_error: input.journal_error.map(SafeText::new),
                journal_schema_version: input.journal_schema_version,
                journal_file_bytes: input.journal_file_bytes,
            },
            sessions,
            providers: input
                .providers
                .into_iter()
                .map(|provider| ProviderDiagnostics {
                    id: SafeText::new(provider.id),
                    protocol: provider.protocol.map(SafeText::new),
                    origin: provider.origin.map(SafeText::new),
                    install_channel: provider.install_channel.map(SafeText::new),
                    installed_version: provider.installed_version.map(SafeText::new),
                    latest_version: provider.latest_version.map(SafeText::new),
                    agent_version: provider.agent_version.map(SafeText::new),
                    installed: provider.installed,
                    authentication: SafeText::new(provider.authentication),
                })
                .collect(),
            environment: EnvironmentDiagnostics {
                os_version: SafeText::new(input.os_version),
                app_version: input.app_version,
                runtime_dir: SafeText::new(input.runtime_dir),
                pipe_name: SafeText::new(input.pipe_name),
                login_shell_capture: input.login_shell_capture,
            },
        }
    }
}

#[cfg(feature = "server")]
fn max_age(current: Option<u64>, candidate: Option<u64>) -> Option<u64> {
    match (current, candidate) {
        (Some(current), Some(candidate)) => Some(current.max(candidate)),
        (None, candidate) => candidate,
        (current, None) => current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{Journal, JOURNAL_SCHEMA_VERSION};
    use devboule_protocol::{
        JournalStats, ProviderInfo, Session, SessionKind, SessionState, TranscriptIntegrity,
        PROTOCOL_VERSION,
    };

    fn session(id: &str, kind: SessionKind, state: SessionState, title: &str) -> Session {
        Session {
            id: id.to_string(),
            workspace_id: None,
            cwd: None,
            kind,
            title: title.to_string(),
            provider: None,
            peer_session_id: None,
            state,
            elapsed_ms: Some(123),
            created_at_ms: 1,
        }
    }

    fn input() -> DiagnosticsInput {
        DiagnosticsInput {
            instance_id: "instance".to_string(),
            daemon_version: "0.1.0".to_string(),
            protocol_version: PROTOCOL_VERSION,
            pid: 7,
            uptime_ms: 8,
            clients: 2,
            daemon_sessions: 3,
            capabilities: vec!["status".to_string()],
            peak_ring_bytes: 9,
            ring_evicted_bytes: 10,
            ring_dropped_frames: 11,
            journal_stats: None,
            journal_error: None,
            journal_schema_version: 5,
            journal_file_bytes: Some(12),
            sessions: Vec::new(),
            providers: vec![ProviderInfo {
                id: "grok".to_string(),
                executable: "grok.exe".to_string(),
                acp_available: true,
                authentication: "unknown".to_string(),
                protocol: Some("acp".to_string()),
                origin: Some("user-binary".to_string()),
                launch_args: None,
                pickable: None,
                installed_version: Some("1.0".to_string()),
                latest_version: Some("1.1".to_string()),
                agent_version: Some("1.0".to_string()),
                install_channel: Some("native".to_string()),
                installed: true,
                npm_package: None,
            }],
            os_version: "Windows".to_string(),
            app_version: "0.1.0".to_string(),
            runtime_dir: "C:\\runtime".to_string(),
            pipe_name: "devboule".to_string(),
            login_shell_capture: LoginShellCaptureOutcome {
                state: crate::login_shell_env::LoginShellCaptureState::NotRun,
                applied_variables: 0,
                preserved_variables: 0,
            },
        }
    }

    fn fixture_report() -> DiagnosticsReport {
        DiagnosticsReport {
            daemon: DaemonDiagnostics {
                version: "0.1.0".to_string(),
                protocol_version: PROTOCOL_VERSION,
                pid: 54_596,
                uptime_ms: 20_651,
                clients: 1,
                sessions: 0,
                capabilities: vec![
                    "ping".to_string(),
                    "status".to_string(),
                    "shutdown".to_string(),
                    "sessions".to_string(),
                    "journal".to_string(),
                    "typed_permissions".to_string(),
                ],
                instance_id: "54596-1788767709103".to_string(),
            },
            health: HealthDiagnostics {
                peak_ring_bytes: 0,
                ring_evicted_bytes: 0,
                ring_dropped_frames: 0,
                journal_stats: Some(JournalStats {
                    accepted_frames: 0,
                    accepted_bytes: 0,
                    committed_frames: 0,
                    committed_bytes: 0,
                    failed_frames: 0,
                }),
                journal_error: None,
                journal_schema_version: 5,
                journal_file_bytes: Some(9_740_288),
            },
            sessions: SessionDiagnostics {
                total: 69,
                live: 0,
                silent: 0,
                ended: 10,
                recovered: 59,
                terminal: 7,
                acp: 57,
                claude: 5,
                pi: 0,
                codex: 0,
                resumable: 28,
                oldest_live_age_ms: None,
            },
            providers: vec![ProviderDiagnostics {
                id: SafeText::new("claude"),
                protocol: Some(SafeText::new("stream-json")),
                origin: Some(SafeText::new("user-binary")),
                install_channel: Some(SafeText::new("native")),
                installed_version: None,
                latest_version: None,
                agent_version: None,
                installed: true,
                authentication: SafeText::new("unknown"),
            }],
            environment: EnvironmentDiagnostics {
                os_version: SafeText::new("Windows 10.0.26200 (x86_64)"),
                app_version: "0.1.0".to_string(),
                runtime_dir: SafeText::new("[redacted-home]\\AppData\\Local\\Devboule"),
                pipe_name: SafeText::new(r"\\.\pipe\devboule-[redacted]"),
                login_shell_capture: LoginShellCaptureOutcome {
                    state: crate::login_shell_env::LoginShellCaptureState::Applied,
                    applied_variables: 12,
                    preserved_variables: 2,
                },
            },
        }
    }

    #[test]
    fn public_constructor_redacts_free_text() {
        let mut source = input();
        let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        source.journal_error = Some(format!(
            "token={secret} C:\\Users\\alice\\project S-1-5-21-111-222-333-1001"
        ));
        source.os_version = "Windows for C:\\Users\\alice".to_string();
        source.runtime_dir = "C:\\Users\\alice\\AppData\\Local\\devboule".to_string();
        source.providers[0].executable = "C:\\Users\\alice\\DEVBOULE_BIN_PATH.exe".to_string();
        source.providers[0].authentication = format!("failed: {secret}");

        let report = DiagnosticsReport::new(source);
        let encoded = serde_json::to_string(&report).expect("report json");
        assert!(!encoded.contains(secret));
        assert!(!encoded.contains("C:\\Users\\alice"));
        assert!(!encoded.contains("S-1-5-21-111-222-333-1001"));
        assert!(!encoded.contains("DEVBOULE_BIN_PATH"));
    }

    #[test]
    fn diagnostics_redact_runtime_pipe_hashes() {
        let runtime_dir = r"C:\Users\gualt\AppData\Local\Devboule";
        let paths = crate::paths::RuntimePaths::from_dir(runtime_dir);
        let hash = paths
            .pipe_name
            .strip_prefix(r"\\.\pipe\devboule-")
            .expect("runtime pipe prefix")
            .to_string();
        assert_eq!(
            SafeText::new(&paths.pipe_name).as_str(),
            r"\\.\pipe\devboule-[redacted]"
        );

        let mut source = input();
        source.runtime_dir = runtime_dir.to_string();
        source.pipe_name = paths.pipe_name;
        let report = DiagnosticsReport::new(source);
        let encoded = serde_json::to_string(&report).expect("report json");

        assert_eq!(hash.len(), 16);
        assert!(
            !encoded.contains(&hash),
            "runtime directory hash leaked in diagnostics: {encoded}"
        );
        assert!(encoded.contains("[redacted]"));
    }

    #[test]
    fn pipe_redactor_replaces_exact_hashes() {
        assert_eq!(
            redact_pipe_names(r"\\.\pipe\devboule-0f1e2d3c4b5a6978"),
            r"\\.\pipe\devboule-[redacted]"
        );
    }

    #[test]
    fn safe_text_redacts_pipe_hashes_when_reading_wire_text() {
        let encoded =
            serde_json::to_string(r"\\.\pipe\devboule-0f1e2d3c4b5a6978").expect("pipe name json");
        let decoded: SafeText = serde_json::from_str(&encoded).expect("safe text");
        assert_eq!(decoded.as_str(), r"\\.\pipe\devboule-[redacted]");
    }

    #[test]
    fn windows_home_paths_redact_drive_and_unc_forms() {
        let cases = [
            (
                r"C:\Users\alice\AppData\Local\Devboule",
                r"[redacted-home]\AppData\Local\Devboule",
            ),
            (
                r"C:/Users/alice/AppData/Local/Devboule",
                r"[redacted-home]/AppData/Local/Devboule",
            ),
            (
                r"\\server\share\Users\alice\AppData\Local\Devboule",
                r"[redacted-home]\AppData\Local\Devboule",
            ),
            (
                r"//server/share/Users/alice/AppData/Local/Devboule",
                r"[redacted-home]/AppData/Local/Devboule",
            ),
            (
                "/Users/alice/Library/Application Support/Devboule",
                "[redacted-home]/Library/Application Support/Devboule",
            ),
            (
                "/home/alice/.config/devboule",
                "[redacted-home]/.config/devboule",
            ),
            ("/Users/alice", "[redacted-home]"),
            ("/home/alice", "[redacted-home]"),
            ("/usr/local/bin", "/usr/local/bin"),
            ("/opt/homebrew/bin", "/opt/homebrew/bin"),
            (
                "/var/folders/ab/cd/T/devboule",
                "/var/folders/ab/cd/T/devboule",
            ),
        ];

        for (raw, expected) in cases {
            assert_eq!(redact_windows_home_paths(raw), expected);
        }
    }

    #[test]
    fn report_includes_login_shell_capture_outcome_without_environment_values() {
        let report = fixture_report();
        let encoded = serde_json::to_value(report).expect("report json");
        assert_eq!(
            encoded["environment"]["loginShellCapture"]["state"],
            "applied"
        );
        assert_eq!(
            encoded["environment"]["loginShellCapture"]["appliedVariables"],
            12
        );
        assert_eq!(
            encoded["environment"]["loginShellCapture"]["preservedVariables"],
            2
        );
        let environment = encoded["environment"].as_object().expect("environment");
        assert!(!environment.contains_key("PATH"));
        assert!(!environment.contains_key("captured"));
    }

    #[test]
    fn public_constructor_redacts_provider_metadata_and_versions() {
        let mut source = input();
        let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        source.providers[0].id = secret.to_string();
        source.providers[0].protocol = Some(secret.to_string());
        source.providers[0].origin = Some(secret.to_string());
        source.providers[0].install_channel = Some(secret.to_string());
        source.providers[0].installed_version = Some(secret.to_string());
        source.providers[0].latest_version = Some(secret.to_string());
        source.providers[0].agent_version = Some(secret.to_string());

        let report = DiagnosticsReport::new(source);
        let encoded = serde_json::to_string(&report).expect("report json");
        assert!(
            !encoded.contains(secret),
            "provider metadata leaked: {encoded}"
        );
    }

    #[test]
    fn report_excludes_titles_and_conversation_payloads() {
        let mut source = input();
        source.sessions = vec![session(
            "s.owner.title",
            SessionKind::Terminal,
            SessionState::Ended {
                generation: 1,
                code: Some(0),
                integrity: TranscriptIntegrity::Complete,
            },
            "USER_TITLE transcript AgentStderr permission env args cwd",
        )];
        let report = DiagnosticsReport::new(source);
        let encoded = serde_json::to_string(&report).expect("report json");
        assert!(!encoded.contains("USER_TITLE"));
        assert!(!encoded.contains("AgentStderr"));
        assert!(!encoded.contains("permission env args cwd"));
    }

    #[test]
    fn aggregate_counts_cover_mixed_sessions() {
        let mut source = input();
        source.sessions = vec![
            session(
                "s.1",
                SessionKind::Terminal,
                SessionState::Live { generation: 1 },
                "one",
            ),
            session(
                "s.2",
                SessionKind::Acp,
                SessionState::Silent { generation: 1 },
                "two",
            ),
            session(
                "s.3",
                SessionKind::Claude,
                SessionState::Ended {
                    generation: 1,
                    code: Some(0),
                    integrity: TranscriptIntegrity::Complete,
                },
                "three",
            ),
            session(
                "s.4",
                SessionKind::Acp,
                SessionState::Recovered {
                    generation: 1,
                    integrity: TranscriptIntegrity::Unverifiable {
                        dropped_frames: 0,
                        dropped_bytes: 0,
                        trimmed_bytes: 0,
                    },
                },
                "four",
            ),
        ];
        source.sessions[3].provider = Some("grok".to_string());
        source.sessions[3].peer_session_id = Some("peer-4".to_string());
        let report = DiagnosticsReport::new(source);
        assert_eq!(report.sessions.total, 4);
        assert_eq!(report.sessions.live, 1);
        assert_eq!(report.sessions.ended, 1);
        assert_eq!(report.sessions.recovered, 1);
        assert_eq!(report.sessions.terminal, 1);
        assert_eq!(report.sessions.acp, 2);
        assert_eq!(report.sessions.claude, 1);
        assert_eq!(report.sessions.resumable, 1);
        assert_eq!(report.health.journal_schema_version, 5);
        assert_eq!(report.health.journal_file_bytes, Some(12));
    }

    #[test]
    fn journal_size_and_schema_are_read_from_the_real_journal() {
        let directory = std::env::temp_dir().join(format!(
            "devboule-diagnostics-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("journal directory");
        let path = directory.join("journal.db");
        let journal = Journal::open(&path).expect("journal");
        let bytes = journal.file_len().expect("journal file size");
        assert!(bytes > 0, "schema-bearing journal should occupy disk");

        let mut source = input();
        source.journal_schema_version = JOURNAL_SCHEMA_VERSION;
        source.journal_file_bytes = Some(bytes);
        let report = DiagnosticsReport::new(source);
        assert_eq!(report.health.journal_schema_version, JOURNAL_SCHEMA_VERSION);
        assert_eq!(report.health.journal_file_bytes, Some(bytes));

        journal.shutdown();
        std::fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn diagnostics_report_fixture_is_the_serialized_wire_contract() {
        const FIXTURE: &str = include_str!("../fixtures/diagnostics-report.json");

        let expected: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture json");
        let report = fixture_report();
        let encoded = serde_json::to_value(&report).expect("report json");
        assert_eq!(encoded, expected);

        let decoded: DiagnosticsReport = serde_json::from_value(expected.clone())
            .expect("fixture must deserialize as DiagnosticsReport");
        let reencoded = serde_json::to_value(decoded).expect("round-trip json");
        assert_eq!(reencoded, expected);
    }
}
