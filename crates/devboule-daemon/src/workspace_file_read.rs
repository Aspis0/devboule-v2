//! The content of one workspace file, for the Files panel's preview — one
//! **window** of it per request. Orchestration only: resolve the folder
//! from a workspace id — never a request field — confine the requested path
//! to it with the same two-layer rule the listing uses (lexical components
//! plus a walk that refuses links and junctions, both through
//! [`crate::workspace_git_support`], so the sentences are shared and not
//! copied), `stat` before reading, and answer with one window: the lines
//! from `from_line`, no more than `line_count` asks and never more bytes
//! than [`FILE_READ_MAX_BYTES`]. A window ends **on a line** whenever the
//! file continues past it — the line straddling the cap becomes the next
//! window's first line, so `from_line + lines` never skips a byte — and a
//! line too big for one window comes back cut, with `truncated`, the
//! `note` that says the rest of it cannot be read this way, and
//! `has_more` false: nothing after it is offered, so nothing after it can
//! be skipped. Locating that first line counts newlines over the prefix
//! chunk by chunk, holding one [`LOCATE_CHUNK`] and never the file's line
//! total — **worst case it reads the whole file in those chunks** (a late
//! line, or one that does not exist), while the window itself never
//! exceeds the cap plus the one byte that says the file goes on.
//! `from_line` numbers lines from 1: zero is the caller's mistake and is
//! refused with this frame's own sentence. An image
//! keeps its old road whole — base64 cut in the middle decodes to nothing,
//! so over the cap it is still withheld with the measure. Nothing in this
//! module writes.

use std::io::Read;
use std::path::{Component, Path};
use std::time::UNIX_EPOCH;

use base64::Engine;
use devboule_protocol::{
    DaemonMessage, WorkspaceFileContent, WorkspaceFileContentKind, WorkspaceFileContentStatus,
};

use crate::workspace_files::{names_git_metadata, DOES_NOT_EXIST, NOT_PART_OF_THE_TREE};
use crate::workspace_git_diff::NOT_A_FILE;
use crate::workspace_git_support::{confined, walk, Walked, OUTSIDE_THE_WORKSPACE};
use crate::ServerState;

/// Bytes of one window this reply hands back. The frame is one JSON line of
/// at most [`devboule_protocol::MAX_FRAME_BYTES`] (1 MiB), and the worst
/// JSON escape is 6 bytes per control byte: 128 KiB × 6 = 768 KiB always
/// fits. The read stops one byte past this cap — that byte is what
/// `has_more` is built from — and the window is cut back to the cap before
/// anything is decoded, so no reply outgrows the frame however big the
/// file is.
const FILE_READ_MAX_BYTES: u64 = 128 * 1024;

/// Bytes read per step while locating a window's first line: each chunk is
/// counted for newlines and dropped, so locating a late window holds this
/// much — never the file, and never a line total for it.
const LOCATE_CHUNK: usize = 8 * 1024;

/// The one sentence of this module that no other panel shows yet: the file
/// could not be opened or read. Static, like the rest — no OS error string
/// travels, because those carry paths.
const READ_FAILED: &str = "the file could not be read";

/// The sentence for a window addressed at line 0: lines are numbered from
/// 1, and a zero is the caller's mistake — refused with these words rather
/// than read as the first window by fiat, which would silently shift every
/// line the caller believes it is reading.
const ZERO_LINE: &str = "lines are numbered from 1; from_line cannot be 0";

/// The sentence a line too big for one window travels with: its rest — and
/// with `has_more: false` everything after it — is not reachable window by
/// window, and this reply says so instead of offering a continuation that
/// would skip bytes. Static, like the rest: no path, no OS error string.
const LINE_EXCEEDS_WINDOW: &str =
    "the line exceeds one window; what follows it in the file cannot be read this way";

/// Extensions handed back as image content — recognized by spelling alone,
/// the way Paseo does it (`service.ts:91-95`), because an image's bytes are
/// binary and would otherwise never be shown. `svg` is absent on purpose:
/// it is text and reads better as text. Shared with
/// [`crate::workspace_file_preview`], whose stage gate needs this list for
/// the same reason; the panel's mirror of every extension the preview draws
/// lives in `src/features/workspace/previewMedia.ts` — an image extension
/// joins that mirror and this list or neither.
pub(crate) const IMAGE_EXTENSIONS: [&str; 9] = [
    "avif", "bmp", "gif", "ico", "jpeg", "jpg", "png", "tiff", "webp",
];

/// Resolve `workspace_id` through the registry — never a request field.
/// `from_line`/`line_count` address the window; both `None` is the first
/// window, which is the whole answer a caller unaware of windows gets.
pub(crate) fn reply(
    state: &ServerState,
    id: u64,
    workspace_id: &str,
    path: &str,
    from_line: Option<u64>,
    line_count: Option<u64>,
) -> DaemonMessage {
    let file = match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => content_of(&root, path, from_line, line_count),
        // The sentence comes from the registry and names no path (see
        // `workspace_cwd`): it is safe to echo on this frame.
        Err(error) => refused(error.message),
    };
    DaemonMessage::WorkspaceFileContent { id, file }
}

/// The content of one file of one workspace, one window of it. Private on
/// purpose: callers outside this module arrive through [`reply`], and the
/// test modules are children.
fn content_of(
    root: &Path,
    requested: &str,
    from_line: Option<u64>,
    line_count: Option<u64>,
) -> WorkspaceFileContent {
    // The folder itself — in any spelling (``, `.`, `./`) — is a folder:
    // not an escape, and no file content to hand back. Checked ahead of the
    // confinement, which would call the empty spelling an escape.
    if Path::new(requested)
        .components()
        .all(|component| matches!(component, Component::CurDir))
    {
        return refused(NOT_A_FILE);
    }
    // Confinement first and without a process: nothing below opens a path
    // this check has not put inside `root`.
    let Some(target) = confined(root, requested) else {
        return refused(OUTSIDE_THE_WORKSPACE);
    };
    // The listing's own guard, before the walk: no stat of the repository's
    // metadata folder happens at all, in any spelling Win32 resolves.
    if names_git_metadata(requested) {
        return refused(NOT_PART_OF_THE_TREE);
    }
    // Lines are numbered from 1, and a zero is the caller's mistake —
    // after the confinement guards on purpose, so every refusal the path
    // earns keeps precedence over this one about the frame.
    let first = match from_line {
        None => 1,
        Some(0) => return refused(ZERO_LINE),
        Some(line) => line,
    };
    // Then the walk: the confinement above trusts the spelling, this trusts
    // the filesystem — every component stat'ed without following, a link
    // refused with the shared sentence, a component with no stat ending the
    // walk in the shared "does not exist".
    match walk(root, requested) {
        Walked::Link(sentence) => refused(sentence),
        Walked::Missing => refused(DOES_NOT_EXIST),
        Walked::Inside(metadata) => {
            if !metadata.is_file() {
                return refused(NOT_A_FILE);
            }
            // An image travels as base64, and base64 cut in the middle
            // decodes to nothing: it is the one road here with no window
            // in it, withheld whole over the cap exactly as before.
            if is_image(requested) {
                return image_of(&target, &metadata);
            }
            match window_of(&target, first, line_count) {
                Err(_) => refused(READ_FAILED),
                Ok(window) => {
                    if window.bytes.contains(&0) {
                        return binary(&metadata);
                    }
                    let lines = window.lines;
                    let has_more = window.has_more;
                    let truncated = window.truncated;
                    match window.into_text() {
                        Some(content) => {
                            windowed(content, first, lines, has_more, truncated, &metadata)
                        }
                        // The UTF-8 half of the sniff, on the window in
                        // hand: not decodable, so binary — and binary on
                        // this wire means no content, never a lossy
                        // re-decode of bytes the caller asked to see.
                        None => binary(&metadata),
                    }
                }
            }
        }
    }
}

/// One window's bytes and the facts about its edges.
struct Window {
    /// The window's text as bytes, already cut to the frame's cap.
    bytes: Vec<u8>,
    /// How many lines those bytes hold (0 past the end of the file).
    lines: u64,
    /// Whether another window follows this one.
    has_more: bool,
    /// Whether the cap cut the window's last line short — the file goes on
    /// inside a line this window did not finish.
    truncated: bool,
}

impl Window {
    /// The window as text, or `None` when its bytes are not text at all. A
    /// cut landing inside a multi-byte character is our doing, not the
    /// file's, and is dropped back to the last whole character —
    /// `truncated` has already said the edge is not a line end. A file
    /// whose own bytes break stays binary: the read's rule from its first
    /// slice.
    fn into_text(self) -> Option<String> {
        let truncated = self.truncated;
        match String::from_utf8(self.bytes) {
            Ok(text) => Some(text),
            Err(error) => {
                let utf8 = error.utf8_error();
                if !truncated || utf8.error_len().is_some() {
                    return None;
                }
                let whole = utf8.valid_up_to();
                let bytes = error.into_bytes();
                std::str::from_utf8(&bytes[..whole]).ok().map(str::to_owned)
            }
        }
    }
}

/// Read the window the request addresses: locate its first line, then read
/// at most the cap plus the one byte that says the file goes on. Locating
/// is the unbounded half — worst case it walks the whole prefix (up to the
/// entire file, when the line is late or absent) in [`LOCATE_CHUNK`] chunks
/// to learn where — or whether — the line is there; the bytes this reply
/// hands back never exceed the cap.
fn window_of(target: &Path, from_line: u64, line_count: Option<u64>) -> std::io::Result<Window> {
    let mut file = std::fs::File::open(target)?;
    let budget = FILE_READ_MAX_BYTES as usize;
    let mut bytes: Vec<u8> = Vec::new();
    // `from_line - 1` newlines stand before the window's first line. Every
    // chunk is counted and dropped; the bytes AFTER the newline that lands
    // the address are where the window begins — and reaching EOF first
    // means the window starts past the file's end, which answers with no
    // lines rather than an error.
    let to_skip = from_line - 1;
    let mut seen: u64 = 0;
    let mut chunk = vec![0u8; LOCATE_CHUNK];
    while seen < to_skip {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        for (index, byte) in chunk[..read].iter().enumerate() {
            if *byte == b'\n' {
                seen += 1;
                if seen == to_skip {
                    bytes.extend_from_slice(&chunk[index + 1..read]);
                    break;
                }
            }
        }
    }
    // One byte PAST the cap, and only that one: it is what `has_more` is
    // built from. The locate chunk above is far smaller than the cap, so
    // this subtraction cannot underflow.
    file.by_ref()
        .take((budget + 1 - bytes.len()) as u64)
        .read_to_end(&mut bytes)?;
    let past_the_cap = bytes.len() > budget;
    if past_the_cap {
        bytes.truncate(budget);
    }
    // No more lines than `line_count` asks for; 0 asks for one — a window
    // of nothing is not a request this frame answers. Fewer newlines than
    // that inside the cap means the cap, not the count, ended the window.
    let mut end = bytes.len();
    if let Some(wanted) = line_count {
        let wanted = wanted.max(1);
        let mut lines_seen: u64 = 0;
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                lines_seen += 1;
                if lines_seen == wanted {
                    end = index + 1;
                    break;
                }
            }
        }
    }
    let (has_more, truncated);
    if past_the_cap {
        // When the file continues past this window, the window ends ON a
        // line: the line straddling the cap is not shown half — it becomes
        // the next window's first line, so `from_line + lines` lands on a
        // line start and no byte is ever shown and then skipped. When NO
        // line completes inside the window, that first line alone is
        // bigger than one window: hand back its cap-bytes, declare the cut
        // (`truncated`, whose `note` is built in `windowed`) and stop —
        // with `has_more` false nothing after it is offered, so nothing
        // after it can be skipped either.
        match bytes[..end].iter().rposition(|byte| *byte == b'\n') {
            Some(last_line_end) => {
                end = last_line_end + 1;
                has_more = true;
                truncated = false;
            }
            None => {
                // No newline in the window — so `line_count` never cut it
                // either: `end` is the whole cap, all of one line.
                has_more = false;
                truncated = true;
            }
        }
    } else {
        // EOF inside the window ends the last line instead of cutting it
        // (it ships whole, however it ends); only a `line_count` cut can
        // leave more behind, and it ends on the newline the next window
        // restarts at.
        has_more = end < bytes.len();
        truncated = false;
    }
    bytes.truncate(end);
    let lines = bytes.iter().filter(|byte| **byte == b'\n').count() as u64
        + u64::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
    Ok(Window {
        bytes,
        lines,
        has_more,
        truncated,
    })
}

/// An image's own road: the whole file as base64, under the cap the frame
/// can carry — the withholding left standing, because a window of base64
/// is not content anyone can decode.
fn image_of(target: &Path, metadata: &std::fs::Metadata) -> WorkspaceFileContent {
    if metadata.len() > FILE_READ_MAX_BYTES {
        return withheld(metadata.len(), stamped(metadata));
    }
    let bytes = match std::fs::read(target) {
        Ok(bytes) => bytes,
        Err(_) => return refused(READ_FAILED),
    };
    // The file can grow between the stat and the read: the cap is
    // re-checked on the bytes in hand.
    let size = bytes.len() as u64;
    if size > FILE_READ_MAX_BYTES {
        return withheld(size, stamped(metadata));
    }
    let base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    answered(WorkspaceFileContentKind::Image, base64, size, metadata)
}

/// An image's bytes came back whole, with the stat's own numbers: no
/// window, so the five window fields stay `null`.
fn answered(
    kind: WorkspaceFileContentKind,
    content: String,
    size: u64,
    metadata: &std::fs::Metadata,
) -> WorkspaceFileContent {
    WorkspaceFileContent {
        status: WorkspaceFileContentStatus::Ok,
        kind: Some(kind),
        content: Some(content),
        size: Some(size),
        modified_at: stamped(metadata),
        error: None,
        from_line: None,
        lines: None,
        has_more: None,
        truncated: None,
        note: None,
    }
}

/// One window of a text file: its lines, the line it starts at, how many
/// lines it holds, whether another follows, and whether the cap cut a line
/// too big for one window — with `note` carrying that cut's sentence.
/// `size` stays the file's own stat — a window measures lines, not
/// the file.
fn windowed(
    content: String,
    from_line: u64,
    lines: u64,
    has_more: bool,
    truncated: bool,
    metadata: &std::fs::Metadata,
) -> WorkspaceFileContent {
    WorkspaceFileContent {
        status: WorkspaceFileContentStatus::Ok,
        kind: Some(WorkspaceFileContentKind::Text),
        content: Some(content),
        size: Some(metadata.len()),
        modified_at: stamped(metadata),
        error: None,
        from_line: Some(from_line),
        lines: Some(lines),
        has_more: Some(has_more),
        truncated: Some(truncated),
        // `truncated` and `note` are one fact in two spellings — the
        // boolean and the sentence that says what the cut costs — built
        // together here so no reply can carry one without the other.
        note: truncated.then(|| LINE_EXCEEDS_WINDOW.to_string()),
    }
}

/// A file read and classified as bytes with no text inside: the stat's
/// numbers travel, the content does not — and no window, because there is
/// no window of bytes to page through.
fn binary(metadata: &std::fs::Metadata) -> WorkspaceFileContent {
    WorkspaceFileContent {
        status: WorkspaceFileContentStatus::Binary,
        kind: Some(WorkspaceFileContentKind::Binary),
        content: None,
        size: Some(metadata.len()),
        modified_at: stamped(metadata),
        error: None,
        from_line: None,
        lines: None,
        has_more: None,
        truncated: None,
        note: None,
    }
}

/// Content withheld for the cap — an image over it, the one case where no
/// window helps: the measure is in the sentence, the real size in `size`,
/// refused whole, never cut short.
fn withheld(size: u64, modified_at: Option<i64>) -> WorkspaceFileContent {
    WorkspaceFileContent {
        status: WorkspaceFileContentStatus::TooLarge,
        kind: None,
        content: None,
        size: Some(size),
        modified_at,
        error: Some(format!(
            "the file is larger than the {FILE_READ_MAX_BYTES}-byte content cap; its content is \
             not handed back"
        )),
        from_line: None,
        lines: None,
        has_more: None,
        truncated: None,
        note: None,
    }
}

/// A refusal to answer: the sentence says what stopped it, and — per the
/// carve-outs on the type — nothing else travels with it.
fn refused(sentence: impl Into<String>) -> WorkspaceFileContent {
    WorkspaceFileContent {
        status: WorkspaceFileContentStatus::Refused,
        kind: None,
        content: None,
        size: None,
        modified_at: None,
        error: Some(sentence.into()),
        from_line: None,
        lines: None,
        has_more: None,
        truncated: None,
        note: None,
    }
}

/// The stat's own mtime, milliseconds since the epoch; `None` when the
/// filesystem gave no stamp. Shared with the preview's stage, which sends
/// the same number for the same file's own stat.
pub(crate) fn stamped(metadata: &std::fs::Metadata) -> Option<i64> {
    let time = metadata.modified().ok()?;
    Some(match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_millis() as i64,
        Err(before) => -(before.duration().as_millis() as i64),
    })
}

/// Whether the requested spelling names an image by its extension — the
/// decision Paseo makes before it sniffs, for the same reason.
fn is_image(requested: &str) -> bool {
    Path::new(requested).extension().is_some_and(|extension| {
        let extension = extension.to_string_lossy().to_ascii_lowercase();
        IMAGE_EXTENSIONS.contains(&extension.as_str())
    })
}

#[cfg(test)]
#[path = "workspace_file_read_fixture.rs"]
mod fixture;
#[cfg(test)]
#[path = "workspace_file_read_tests.rs"]
mod tests;
/// The window's own cases — the first one of a big file, the continuation
/// at its edge, the last one, the one past the end, the line the cap cuts —
/// split by subject, not by line count: the refusals and the classification
/// of the bytes are in [`tests`].
#[cfg(test)]
#[path = "workspace_file_read_window_tests.rs"]
mod window_tests;
