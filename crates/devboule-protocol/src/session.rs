//! Session wire types. `SessionKind`, `Session`, and `SessionEvent` are the
//! M2 contract; moving them here must not change the JSON the TypeScript
//! frontend already consumes.

use serde::{Deserialize, Serialize};

use crate::error::{ErrorCode, ErrorDetails, WireError};

/// M2 implements Terminal; ACP/Agent can be added as another serialized
/// variant without changing the command signatures or the existing
/// `terminal` wire value.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Terminal,
    Acp,
}

/// Public session metadata returned by `session_create` and `sessions_list`.
///
/// `workspace_id` is optional in M2 because workspace lookup is not
/// implemented yet; the terminal starts in the app process's current
/// directory.
///
/// `state` is the type-system distinction between a live process and a
/// recovered transcript. A recovered session is not a live one with a
/// comment: it cannot accept input, and attaching to it replays a journal.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub workspace_id: Option<String>,
    pub kind: SessionKind,
    pub title: String,
    pub state: SessionState,
}

/// How this session currently exists. Live and recovered are different
/// kinds of thing: one has a process, the other is a transcript of a
/// process the daemon can no longer see.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SessionState {
    /// A process is running. Output is live.
    Live { generation: u64 },
    /// The process exited while this daemon was alive. `code` is the
    /// observed exit status (`None` if the child did not report one).
    Ended { generation: u64, code: Option<u32> },
    /// The daemon that owned the process is gone (kill, crash, update).
    /// Replay only. `truncated` is set when the journal dropped frames
    /// (slow or full disk) and the transcript is a prefix, not a lie.
    Recovered { generation: u64, truncated: bool },
}

impl SessionState {
    pub fn generation(&self) -> u64 {
        match *self {
            Self::Live { generation }
            | Self::Ended { generation, .. }
            | Self::Recovered { generation, .. } => generation,
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live { .. })
    }
}

/// Events sent over the Tauri Channel supplied to `session_attach`.
///
/// `seq` starts at 1 and is contiguous for output chunks in one
/// *generation* of a session unless an [`SessionEvent::OutputGap`] declares
/// a missing range. A cursor means "the last output sequence received or
/// explicitly accounted for by a gap"; replay therefore sends chunks whose
/// sequence is strictly greater than `from_cursor`.
///
/// Permission and ACP variants are intentionally reserved for M6: adding
/// new tagged variants is additive for consumers that ignore unknown event
/// types. M3.5 uses that freedom: [`SessionEvent::Snapshot`] delivers the
/// current screen state on attach instead of a replay of past frames.
///
/// This enum is the TypeScript `SessionEvent` contract. Generation is **not**
/// a field here: it lives on [`Cursor`] and on [`super::SessionEventEnvelope`]
/// so a reconnecting client can tell a recreated process from the stream it
/// left. Putting generation on every output chunk would change the Channel
/// payload the frontend already parses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    Output {
        seq: u64,
        data: String,
    },
    /// Process was observed to exit while the daemon was alive.
    Exit {
        code: Option<u32>,
    },
    /// Journal replay of a session whose process died with the daemon.
    /// Distinct from [`SessionEvent::Exit`]: Exit means the process was
    /// seen to die; Recovered means it was not, and this is a transcript.
    Recovered {
        truncated: bool,
    },
    /// The journal has started dropping output for this live session.
    JournalDegraded,
    /// Output in this sequence range is no longer available to replay.
    ///
    /// The range is declared in-band so a client can mark the transcript
    /// incomplete instead of presenting a silently corrupted terminal.
    OutputGap {
        #[serde(rename = "fromSeq")]
        from_seq: u64,
        #[serde(rename = "toSeq")]
        to_seq: u64,
        #[serde(rename = "droppedBytes")]
        dropped_bytes: u64,
        #[serde(rename = "droppedFrames")]
        dropped_frames: u64,
    },
    /// Current screen state, delivered on attach instead of a replay of
    /// past frames (M3.5). The daemon holds a headless terminal emulator,
    /// applies every output chunk to it in sequence order, and renders the
    /// visible grid to a canonical ANSI string; the client writes `data`
    /// into its terminal emulator, then restores the cursor and all state from
    /// the explicit metadata below, and only then releases input. Output chunks
    /// that arrive after the snapshot are ordinary live events with sequences
    /// strictly greater than `as_of_seq`.
    Snapshot {
        /// Sequence boundary of this snapshot.
        ///
        /// A snapshot carrying `as_of_seq = N` is exactly the emulator
        /// state after every output chunk with sequence `<= N` has been
        /// applied, and before any chunk with sequence `> N`. Every live
        /// event after it carries a sequence strictly greater than `N`.
        ///
        /// The boundary is on **application to the emulator** — not on the
        /// write to the pipe, not on the journal commit, not on receipt by
        /// the client. The previous design advanced its cursor at
        /// pipe-write time, which let it claim delivered what was only
        /// queued; reconnections then produced both duplicates (queued
        /// chunks replayed) and gaps (queued chunks counted as seen and
        /// never sent). On the daemon side, capturing this state and
        /// registering a new attachment must happen under the same lock,
        /// or output applied in between lands in neither the snapshot nor
        /// the queued stream.
        #[serde(rename = "asOfSeq")]
        as_of_seq: u64,
        /// Screen width, in columns, the snapshot was taken at.
        cols: u16,
        /// Screen height, in rows, the snapshot was taken at.
        rows: u16,
        /// Reconstructed ANSI/VT string that reproduces the visible screen
        /// at `cols` x `rows`. Deliberately a rendered string, not a cell
        /// grid: a typical 200x50 screen is 8-30 KiB as ANSI against
        /// 470-850 KiB as JSON cells. The worst case still serialises under
        /// the 1 MiB NDJSON frame cap (`MAX_FRAME_BYTES`), but it is orders
        /// of magnitude larger than an ordinary output frame — see the
        /// frame-cap test in this module before assuming snapshots are
        /// small.
        data: String,
        /// Screen cursor to restore after writing `data`.
        cursor: ScreenCursor,
        /// The alternate screen buffer was active at capture. The client
        /// must restore this mode before releasing input.
        #[serde(rename = "alternateScreen")]
        alternate_screen: bool,
        /// Bracketed paste (DECSET 2004) was enabled at capture. The
        /// client must restore this mode before releasing input.
        #[serde(rename = "bracketedPaste")]
        bracketed_paste: bool,
        /// Whether line wrapping was enabled at capture.
        #[serde(rename = "lineWrap")]
        line_wrap: bool,
        /// Window title at capture, when the daemon saw one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
}

/// Shape of the screen cursor carried by [`SessionEvent::Snapshot`].
///
/// The wire values are the cursor styles xterm.js accepts, so the client
/// can apply the shape without translating it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

/// Cursor state of the captured screen.
///
/// Zero-based: row 0 is the first row of the visible screen, col 0 the
/// first column. Not a [`Cursor`]: that one is a replay position in the
/// output sequence, this one is a place on the screen.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScreenCursor {
    pub row: u16,
    pub col: u16,
    pub visible: bool,
    pub shape: CursorShape,
    pub blinking: bool,
}

/// Replay position for a reconnecting client.
///
/// `seq` is the last output sequence the client has accounted for **for
/// `generation`**, including ranges explicitly declared unavailable by an
/// [`SessionEvent::OutputGap`].
/// A session whose process died and was recreated MUST bump `generation`
/// so a client holding an old cursor cannot silently consume a different
/// stream as if it were a continuation.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    pub generation: u64,
    pub seq: u64,
}

/// Outcome of a typed permission prompt. Reserved for M6; the wire name is
/// fixed so permission-response idempotency can be specified now.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOutcome {
    AllowOnce,
    Deny,
}

/// ACP persistence handle. Terminal sessions always use [`PersistenceKind::None`].
///
/// The protocol carries an explicit "resume not supported" result because
/// "ACP is spoken" does not imply "resume is spoken" (ARCHITETTURA §1.6).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Persistence {
    pub kind: PersistenceKind,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PersistenceKind {
    None,
    Acp { handle: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResumeResult {
    Resumed { session: Session },
    NotSupported,
    Failed { message: String },
}

/// Decide whether `cursor` may replay against `current_generation`.
///
/// Same generation: replay chunks with `seq > cursor.seq`.
/// Different generation: error. The client must treat the stream as new
/// (typically `Cursor { generation: current, seq: 0 }`).
pub fn cursor_replay_ok(current_generation: u64, cursor: Cursor) -> Result<(), WireError> {
    if cursor.generation == current_generation {
        Ok(())
    } else {
        Err(WireError {
            id: None,
            code: ErrorCode::SessionGenerationMismatch,
            message: format!(
                "session generation is {}, client cursor is {}",
                current_generation, cursor.generation
            ),
            details: Some(ErrorDetails::GenerationMismatch {
                current: current_generation,
                requested: cursor.generation,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_event_uses_a_type_tag() {
        let output = serde_json::to_value(SessionEvent::Output {
            seq: 7,
            data: "hi".to_string(),
        })
        .expect("json");
        assert_eq!(output["type"], "output");
        assert_eq!(output["seq"], 7);
        let exit = serde_json::to_value(SessionEvent::Exit { code: Some(0) }).expect("json");
        assert_eq!(exit["type"], "exit");
        assert_eq!(exit["code"], 0);
    }

    #[test]
    fn session_uses_camel_case_workspace_id() {
        let session = Session {
            id: "session-1-1".to_string(),
            workspace_id: Some("ws-1".to_string()),
            kind: SessionKind::Terminal,
            title: "Terminal".to_string(),
            state: SessionState::Live { generation: 1 },
        };
        let value = serde_json::to_value(&session).expect("json");
        assert_eq!(value["workspaceId"], "ws-1");
        assert_eq!(value["kind"], "terminal");
        assert!(value.get("generation").is_none());
        assert_eq!(value["state"]["type"], "live");
        assert_eq!(value["state"]["generation"], 1);
    }

    #[test]
    fn recovered_is_a_different_wire_type_from_live_and_ended() {
        let live = serde_json::to_value(SessionState::Live { generation: 1 }).expect("json");
        let ended = serde_json::to_value(SessionState::Ended {
            generation: 1,
            code: Some(0),
        })
        .expect("json");
        let recovered = serde_json::to_value(SessionState::Recovered {
            generation: 1,
            truncated: false,
        })
        .expect("json");
        assert_eq!(live["type"], "live");
        assert_eq!(ended["type"], "ended");
        assert_eq!(recovered["type"], "recovered");
        assert_ne!(live["type"], recovered["type"]);
        assert_ne!(ended["type"], recovered["type"]);
    }

    #[test]
    fn recovered_event_is_distinct_from_exit() {
        let recovered =
            serde_json::to_value(SessionEvent::Recovered { truncated: true }).expect("json");
        let exit = serde_json::to_value(SessionEvent::Exit { code: None }).expect("json");
        assert_eq!(recovered["type"], "recovered");
        assert_eq!(recovered["truncated"], true);
        assert_eq!(exit["type"], "exit");
        assert_ne!(recovered["type"], exit["type"]);
    }

    #[test]
    fn journal_degraded_event_round_trips() {
        let event = SessionEvent::JournalDegraded;
        let encoded = serde_json::to_value(&event).expect("json");
        assert_eq!(encoded["type"], "journal_degraded");
        let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
        assert_eq!(decoded, event);
    }

    #[test]
    fn output_gap_event_round_trips_with_sequence_range() {
        let event = SessionEvent::OutputGap {
            from_seq: 4,
            to_seq: 9,
            dropped_bytes: 123,
            dropped_frames: 6,
        };
        let encoded = serde_json::to_value(&event).expect("json");
        assert_eq!(encoded["type"], "output_gap");
        assert_eq!(encoded["fromSeq"], 4);
        assert_eq!(encoded["toSeq"], 9);
        assert_eq!(encoded["droppedBytes"], 123);
        assert_eq!(encoded["droppedFrames"], 6);
        let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
        assert_eq!(decoded, event);
    }

    #[test]
    fn snapshot_event_round_trips_with_camel_case_wire_names() {
        let event = SessionEvent::Snapshot {
            as_of_seq: 41,
            cols: 200,
            rows: 50,
            data: "\u{1b}[2J\u{1b}[H$ ls\r\nsrc  target\r\n".to_string(),
            cursor: ScreenCursor {
                row: 12,
                col: 34,
                visible: true,
                shape: CursorShape::Block,
                blinking: true,
            },
            alternate_screen: false,
            bracketed_paste: true,
            line_wrap: true,
            title: Some("devboule - pwsh".to_string()),
        };
        let encoded = serde_json::to_value(&event).expect("json");
        assert_eq!(encoded["type"], "snapshot");
        assert_eq!(encoded["asOfSeq"], 41);
        assert_eq!(encoded["cols"], 200);
        assert_eq!(encoded["rows"], 50);
        assert_eq!(encoded["cursor"]["row"], 12);
        assert_eq!(encoded["cursor"]["col"], 34);
        assert_eq!(encoded["cursor"]["visible"], true);
        assert_eq!(encoded["cursor"]["shape"], "block");
        assert_eq!(encoded["cursor"]["blinking"], true);
        assert_eq!(encoded["alternateScreen"], false);
        assert_eq!(encoded["bracketedPaste"], true);
        assert_eq!(encoded["lineWrap"], true);
        assert_eq!(encoded["title"], "devboule - pwsh");
        // A rename here silently breaks the client, so pin the exact key
        // set, not just the individual names.
        let mut keys: Vec<&str> = encoded
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "alternateScreen",
                "asOfSeq",
                "bracketedPaste",
                "cols",
                "cursor",
                "data",
                "lineWrap",
                "rows",
                "title",
                "type"
            ]
        );
        let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
        assert_eq!(decoded, event);
    }

    #[test]
    fn snapshot_title_is_absent_when_none() {
        let event = SessionEvent::Snapshot {
            as_of_seq: 0,
            cols: 80,
            rows: 24,
            data: "\u{1b}[Hready".to_string(),
            cursor: ScreenCursor {
                row: 0,
                col: 5,
                visible: true,
                shape: CursorShape::Underline,
                blinking: false,
            },
            alternate_screen: false,
            bracketed_paste: false,
            line_wrap: false,
            title: None,
        };
        let encoded = serde_json::to_value(&event).expect("json");
        assert!(encoded.get("title").is_none());
        let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
        assert_eq!(decoded, event);
    }

    #[test]
    fn cursor_shapes_have_stable_wire_values() {
        assert_eq!(
            serde_json::to_value(CursorShape::Block).expect("json"),
            "block"
        );
        assert_eq!(
            serde_json::to_value(CursorShape::Underline).expect("json"),
            "underline"
        );
        assert_eq!(serde_json::to_value(CursorShape::Bar).expect("json"), "bar");
        for shape in [CursorShape::Block, CursorShape::Underline, CursorShape::Bar] {
            let decoded: CursorShape =
                serde_json::from_value(serde_json::to_value(shape).expect("json")).expect("shape");
            assert_eq!(decoded, shape);
        }
    }

    #[test]
    fn snapshot_screen_data_is_ndjson_safe() {
        let event = SessionEvent::Snapshot {
            as_of_seq: 7,
            cols: 80,
            rows: 24,
            data: "row1\r\nrow2\n\u{1b}[K".to_string(),
            cursor: ScreenCursor {
                row: 1,
                col: 0,
                visible: true,
                shape: CursorShape::Block,
                blinking: true,
            },
            alternate_screen: false,
            bracketed_paste: false,
            line_wrap: true,
            title: None,
        };
        let encoded = serde_json::to_string(&event).expect("json");
        assert!(
            !encoded.contains('\n'),
            "compact JSON must not contain a raw newline or NDJSON framing splits the event"
        );
        let decoded: SessionEvent = serde_json::from_str(&encoded).expect("parse");
        assert_eq!(decoded, event);
    }

    #[test]
    fn dense_worst_case_snapshot_still_fits_the_frame_cap() {
        // Heaviest plausible snapshot: a 200x50 screen where every cell
        // repaints foreground and background in its own 24-bit colour, so
        // no pair of SGR sequences can collapse. Each cell costs 49 bytes
        // of escaped JSON (each ESC becomes \u001b), about 490 KiB total -
        // heavier than the ~410 KiB worst case measured for ARCHITETTURA
        // 9.2, and still under the 1 MiB NDJSON frame cap. A typical
        // snapshot is 8-30 KiB: never assume snapshots are always small.
        let mut data = String::with_capacity(200 * 50 * 39);
        for cell in 0..(200u32 * 50) {
            // 100..=255 so every colour component is three digits long.
            let fg = (100 + (cell % 156)) as u8;
            let bg = (100 + (cell * 7 % 156)) as u8;
            data.push_str(&format!(
                "\u{1b}[38;2;{fg};{fg};{fg}m\u{1b}[48;2;{bg};{bg};{bg}mX"
            ));
        }
        let event = SessionEvent::Snapshot {
            as_of_seq: 4096,
            cols: 200,
            rows: 50,
            data,
            cursor: ScreenCursor {
                row: 49,
                col: 199,
                visible: true,
                shape: CursorShape::Bar,
                blinking: false,
            },
            alternate_screen: true,
            bracketed_paste: true,
            line_wrap: true,
            title: None,
        };
        let encoded = serde_json::to_vec(&event).expect("json");
        assert!(
            encoded.len() > 400_000,
            "expected a dense worst-case payload, got {} bytes",
            encoded.len()
        );
        assert!(
            encoded.len() < crate::MAX_FRAME_BYTES,
            "worst-case snapshot serialises to {} bytes, frame cap is {}",
            encoded.len(),
            crate::MAX_FRAME_BYTES
        );
    }

    #[test]
    fn cursor_generation_mismatch_is_an_error() {
        let err = cursor_replay_ok(
            2,
            Cursor {
                generation: 1,
                seq: 9,
            },
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::SessionGenerationMismatch);
        cursor_replay_ok(
            2,
            Cursor {
                generation: 2,
                seq: 9,
            },
        )
        .expect("same generation");
    }

    #[test]
    fn resume_not_supported_is_an_explicit_variant() {
        let value = serde_json::to_value(ResumeResult::NotSupported).expect("json");
        assert_eq!(value["type"], "not_supported");
    }
}
