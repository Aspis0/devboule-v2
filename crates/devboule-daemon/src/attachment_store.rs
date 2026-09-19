//! Where the bytes attached to a prompt live on disk.
//!
//! Files live under the daemon's runtime directory, one folder per session, and
//! never inside the workspace. A workspace is a git checkout the user reads from
//! `git status` and the Changes panel; an attachment written there would appear
//! as the user's own edit and could be committed by accident.
//!
//! Layout is `<runtime dir>/attachments/<session id>/<sha256>.<ext>`.
//!
//! The file name is the sha256 of the bytes that were written, plus an
//! extension taken from the MIME type. For an SVG those are the decoded bytes;
//! for a JPEG or a PNG they are the decoded bytes with their identity metadata
//! removed ([`crate::raster_metadata`]), so the name answers for what is on
//! disk rather than for what arrived. The user's file name is not in the path,
//! for two reasons. It comes from outside — it may contain `..`, a path
//! separator, or a drive letter — and the digest contains none of those, so
//! traversal is not possible to express. And a content-addressed name makes
//! re-materializing the same image reuse the file: the same bytes are the same
//! path, so a second turn with the same picture, or a replay of the history
//! that rebuilds it, does not leave another copy behind.
//!
//! # The store's byte budget
//!
//! Every file in this store counts against [`MAX_ATTACHMENT_OWNER_BYTES`], and
//! "this store" is one account's store: the runtime directory is
//! `%LOCALAPPDATA%\Devboule` (`crate::paths::RuntimePaths::from_env`, with
//! `from_dir` for a test runtime and a `DEVBOULE_RUNTIME_DIR` override), so the
//! store is per-user by construction and the budget needs no key at all. The
//! constant's name is the wire's; the quantity is the store's.
//!
//! There was a key once, and it was wrong. The middle segment of a session id
//! looked like an owner and is not one: `compose_session_id` fills it from
//! `OwnerId::session_token`, which is one *connection's* client token cut to
//! sixteen characters. One user running two clients would have had two budgets
//! of twenty megabytes, and two clients whose tokens share sixteen leading
//! characters would have shared one. Nothing here parses a session id for
//! anything but a folder name.
//!
//! The total is held in memory ([`StoreState`]) rather than walked per question:
//! a forty-page deck is two hundred deposits, and walking the store under its
//! single write lock would put every session's attachment work behind that walk
//! (D4 of `DECIDE-deposit-open-questions`). The tree stays the truth. The cache
//! is derived from it, built by one walk the first time a budget is needed, and
//! moved by the same guard that moves the files. A folder that walk could not
//! read makes the total unknown rather than zero, and an unknown total is
//! refused: a number below the truth admits the bytes the limit exists to
//! refuse.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use base64::Engine;
use devboule_protocol::{
    invalid_attachment_digest_message, invalid_base64_message, unsupported_attachment_type_message,
    validate_session_id, ErrorCode, PromptAttachment, WireError, MAX_ATTACHMENT_OWNER_BYTES,
};
use sha2::{Digest, Sha256};

use crate::raster_metadata::{
    sniff_raster_mime, strip_raster_metadata, RasterMime, RasterStripError,
};

/// The runtime-dir subdirectory that holds every session's attachments.
const ATTACHMENTS_DIR: &str = "attachments";

/// How long a session's attachment folder may outlive its newest write.
///
/// Session close removes the folder outright, so this exists for the close that
/// never ran: a daemon killed with the power failed, a crash, a machine that
/// went down mid-session. Seven days with no write means no session is using it.
pub(crate) const ATTACHMENT_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The number of hex characters a SHA-256 digest has (32 bytes).
///
/// The protocol states the same length for a reference's digest, and the store
/// states it again for its own reason: this is the rule that decides whether a
/// string may be joined to a path at all. See `is_digest`.
const DIGEST_HEX_LEN: usize = 64;

/// The file extensions this store writes, one per supported type.
///
/// The values of `extension_for`, named on their own for the two places that
/// start from what is on disk rather than from a MIME type: a file named after
/// a digest carries an extension, and nothing that names it carries a type.
///
/// It has to grow whenever `extension_for` grows, and forgetting it is silent
/// in the direction that hurts: the write succeeds, the budget is charged, and
/// `find_stored` — which reads *this* table — never returns the file again.
/// `text/markdown` landed in `ATTACHMENT_MIME_TYPES` and `extension_for` and
/// not here, which left every deposited artifact unresolvable through the
/// listing. `a_deposited_markdown_artifact_resolves_by_digest` is the test that
/// notices, because it deposits and then resolves instead of counting entries.
const STORED_EXTENSIONS: [&str; 4] = ["png", "jpg", "svg", "md"];

/// What the store's write lock carries besides the exclusion itself: the bytes
/// each session folder holds, which is what the store's total is summed from.
///
/// It is held inside that lock rather than behind a mutex of its own, and that
/// is a decision rather than a convenience. The invariant is that a file and
/// the total that counts it are never observed apart: the increment happens in
/// the critical section that wrote the file, and `remove_dir_all` drops the
/// session's entry in the critical section that deleted its folder. A second
/// mutex would need an order relative to this one, `std::sync::Mutex` is
/// neither reentrant nor ordered, and both orders would have to be kept by hand
/// in every path that took both — with a hang on the one path that got it
/// wrong. One guard cannot be taken twice, in two orders or any order.
#[derive(Default)]
struct StoreState {
    /// Session id -> what that session's folder holds.
    ///
    /// Keyed per session rather than kept as one running total, because every
    /// deletion path here removes a whole folder (`remove_session`, and the
    /// retention sweep through `remove_if_still_older_than`) and both have to
    /// *report* what left with it: a caller releasing a reservation per device
    /// needs the number, and one running total cannot say what a folder held.
    /// `remove_session` and `sweep_older_than` read it back from here.
    ///
    /// An id that is absent is a session with no folder, which is a known zero.
    /// [`SessionBytes::Unknown`] is a folder that is there and could not be
    /// listed. Those two are not the same fact, and the type is what keeps them
    /// apart: a sentinel count would have to be a number no file can reach, and
    /// the first file to reach it would be a silent grant of budget.
    sessions: HashMap<String, SessionBytes>,
    /// Whether the store root itself could not be listed.
    ///
    /// The same finding as [`SessionBytes::Unknown`] one level up: when the root
    /// cannot be listed, the walk never learns which session folders exist, so
    /// no total is knowable and every budget question answers "unknown" until a
    /// walk succeeds. A root that is *missing* is not this — no store
    /// yet is a store with nothing in it, which is the normal state of a fresh
    /// install.
    root_unreadable: bool,
    /// Whether `sessions` is the whole picture, for this process.
    ///
    /// Set only by a walk that read every folder it saw, and never cleared on
    /// its own, so a complete walk runs at most once per store: a second caller
    /// inside the lock finds the work already done instead of starting a second
    /// walk. The flag is not "the map is non-empty", because a write can land
    /// and be counted before the first budget question is ever asked. A walk
    /// that could not read a folder deliberately leaves it `false`, so the next
    /// budget question walks again and a transient failure heals itself.
    seeded: bool,
}

/// What one session folder holds, as far as the cache can tell.
///
/// An enum rather than `Option<u64>`, because these values are read back out of
/// a map: `get` on `Option<u64>` returns `Option<&Option<u64>>`, two levels that
/// read alike, and `Entry::or_default` on it hands a writer the value that means
/// *unknown* — a trap left for whoever next charges a write to a session.
/// Naming both states makes every read a match with an arm for each, and the
/// writer has to say which one it means.
enum SessionBytes {
    /// The folder was listed and its files counted.
    Known(u64),
    /// The folder is there and could not be listed, so its bytes cannot be
    /// counted. Not zero: zero is a claim about a folder, and this is the
    /// absence of one.
    Unknown,
}

/// The attachment folders of one runtime directory.
#[derive(Clone)]
pub(crate) struct AttachmentStore {
    root: PathBuf,
    /// Whether this store's root is a folder of its own, and every folder under
    /// it carries the current user's DACL.
    ///
    /// False when the root's name is a link or a junction: everything under it
    /// is somebody else's tree, and every path this store builds starts at the
    /// root. False as well when the open could not narrow the DACL of the root
    /// or of a folder that was already there ([`harden_and_sweep_in_store`]):
    /// a folder whose DACL is not this user's is a folder another account can
    /// read, and `resolve` reads a folder before any write has been through it.
    /// It is one field rather than a check per call because it is a property of
    /// the root as this store found it, and the open is the one moment nothing
    /// else is writing into it — the same moment the walk runs in. What it
    /// closes, path by path, is on [`AttachmentStore::new`].
    available: bool,
    /// Serializes writes across sessions. One process owns the runtime dir
    /// (single-instance lock), so this is enough to keep two client threads
    /// materializing the same image from racing over the same temp file. It is
    /// also the lock the store's budget cache is taken with ([`StoreState`]), so
    /// a write and the total it moves are taken and left together.
    write_lock: Arc<Mutex<StoreState>>,
}

/// The names Windows resolves to a device rather than to a file, wherever they
/// appear: writes to `NUL` go nowhere, `CON` is the console.
///
/// Checked on every platform. A folder named `AUX` is legal on POSIX, so the
/// list could have been `#[cfg(windows)]` — but then one id would mean two
/// things depending on the machine, and a deposit would land in a real folder
/// on one and in nothing on the other. The runtime directory this store lives
/// in is a Windows path (`%LOCALAPPDATA%\Devboule`), so Windows is the platform
/// that has to be right, and one rule everywhere is one rule to check.
const RESERVED_DEVICE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Whether `session_id` is a folder name this store will join to its root.
///
/// This is the store's own rule, and it is *not* a copy of the wire's
/// `validate_session_id` (`devboule-protocol/src/ids.rs`). It has two halves,
/// and which half a rule lands in is the whole design:
///
/// *What an identifier is* belongs to the protocol, and is asked for rather
/// than restated. [`validate_session_id`] owns the alphabet (`[A-Za-z0-9._-]`),
/// the length cap and the empty case. A second copy of that answer here — the
/// same alphabet, the same `64` — is exactly how a store and a wire drift: each
/// copy is consistent with itself, so a suite that exercises one proves nothing
/// about the other, and the day the protocol widens its alphabet this store
/// starts refusing ids the daemon composes. Asking is not a formality either:
/// the store is a second door and checks rather than trusts — the same
/// relationship `is_digest` has to the wire's digest check — but it checks with
/// the same function, not with a copy of it.
///
/// *What a path component may be* belongs to the store, because the store is the
/// only thing in this process that turns an id into a path. The protocol has no
/// reason to know any of it, and would be wrong to: a bare `.` is a fine
/// identifier and a hostile folder name.
///
/// - `.` and `..` are the parent directory in both spellings, and both pass the
///   protocol's rules today — asserted in
///   `every_id_the_daemon_composes_is_a_folder_name` rather than assumed, since
///   the rule's necessity rests on it.
/// - A reserved device name, tested on the part before the first dot, because
///   Windows reserves `NUL` and `NUL.txt` and `NUL.tar.gz` alike.
///   See [`RESERVED_DEVICE_NAMES`].
/// - A name ending in `.`. Windows strips a trailing dot when the path is
///   created, so `s.a.1.` and `s.a.1` would be one folder under two cache keys:
///   the cache would charge it twice while the walk, which reads the name the
///   filesystem kept, counts it once.
/// - A name carrying an ASCII upper-case byte, which is the same collision one
///   step further out. Windows resolves folder names case-insensitively while
///   the cache is keyed by the id as written, so `s.a.1` and `S.A.1` are one
///   folder on the disk and two keys in the map: the budget would charge that
///   folder twice, and closing either session would delete the other's files.
///   Every id the daemon mints is lower-case — `compose_session_id` fills the
///   middle segment from a `process-<pid>`/`app-<pid>`/`client`/`daemon` token
///   and the last from the counter and the process nonce
///   (`format!("{counter:08x}-{nonce:016x}")`, `session.rs`) — so this refuses
///   nothing the daemon composes, which is asserted rather than assumed in
///   `every_id_the_daemon_composes_is_a_folder_name`. Folding the name to lower
///   case instead would be the same mistake from the other side: it merges two
///   distinct sessions into one folder on a case-sensitive filesystem, and the
///   id is a name this store does not get to reinterpret.
///
/// What the alphabet buys, and why it has to be the protocol's answer rather
/// than a locally convenient one: no path separator, so a join can only append;
/// no `:`, so there is no drive-relative `C:x` and no NTFS alternate data stream
/// (`file:stream`); no `\`, which is half of the leading `\\` a UNC path needs.
/// An id that is a *name* is appended to the root — the property the
/// two-refusal rule this replaces never established.
fn is_session_folder_name(session_id: &str) -> bool {
    if validate_session_id(session_id).is_err() {
        return false;
    }
    if session_id == "." || session_id == ".." || session_id.ends_with('.') {
        return false;
    }
    if session_id.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return false;
    }
    let stem = session_id.split('.').next().unwrap_or(session_id);
    !RESERVED_DEVICE_NAMES
        .iter()
        .any(|name| stem.eq_ignore_ascii_case(name))
}

impl AttachmentStore {
    /// Open the store over one runtime directory.
    ///
    /// # The root's own name is checked first
    ///
    /// The store's root is the one folder every other path here starts at, so a
    /// link or junction at its name is not one hole among the redirects the rest
    /// of this file refuses: it is the store being somebody else's tree, one
    /// level above every path this file already guards. What is refused while
    /// the root is one, and why refusing beats guarding each call:
    ///
    /// - The scratch sweep below runs here, before anything has been written,
    ///   and it deletes `*.tmp`/`*.bak` files. Through a junction that sweep is
    ///   a deletion in another tree, so the walk returns before its first
    ///   entry (`harden_and_sweep_in_store`, which is also the walk that applies
    ///   the DACL).
    /// - [`AttachmentStore::session`] hands back no folder, which is what makes
    ///   `deposit`, `resolve` and `remove_session` refusals rather than reads and
    ///   writes into that tree — the shape the id rule already uses, and the
    ///   reason there is no per-call check to forget.
    /// - [`AttachmentStore::sweep_older_than`] finds no candidates, so retention
    ///   cannot `remove_dir_all` in it.
    /// - [`AttachmentStore::store_bytes`] and [`AttachmentStore::session_bytes`]
    ///   answer `None`. Never a number: a walk of a junction reports the bytes
    ///   of the tree it names, and a total that counts somebody else's files
    ///   would be the budget this store's limit is enforced against.
    ///
    /// # The walk this open runs
    ///
    /// The store's open, and the one moment a walk of every folder is
    /// certain not to race a write: this instance does not exist yet, and the
    /// daemon holds the single-instance lock on the runtime directory
    /// (`crate::lock`, and the struct comment above), so no other process is
    /// writing into this root either. The walk does two things — the scratch a
    /// killed process left behind, which every later walk would otherwise charge
    /// to the budget for the life of the folder and which no `resolve` can ever
    /// name (see [`discard_scratch`]), and the DACL of every folder it sees,
    /// because a folder that predates this store is a folder no write has been
    /// through and `resolve` reads before the first write.
    ///
    /// The two halves answer for themselves. The scratch sweep is best effort:
    /// it removes this store's own leftovers, and one that survives costs budget
    /// until the next write into that folder. A DACL that could not be applied is
    /// not: the folder keeps a DACL another account may be able to reach, and
    /// `resolve` would read attachments out of it — so the walk answers `false`
    /// and this open marks the store unavailable, which is the same refusal the
    /// list above describes and the field's second reason.
    pub(crate) fn new(runtime_dir: &Path) -> Self {
        let root = runtime_dir.join(ATTACHMENTS_DIR);
        let walked = harden_and_sweep_in_store(&root);
        // A root that is a link is refused for its own reason, so the walk's
        // answer is only consulted when the root is the store's own folder.
        let available = !is_redirect(&root) && walked;
        Self {
            root,
            available,
            write_lock: Arc::new(Mutex::new(StoreState::default())),
        }
    }

    /// The folder holding one session's attachments.
    ///
    /// `None` for anything that is not a folder name this store will use, which
    /// is the whole of [`is_session_folder_name`] and is deliberately stricter
    /// than the wire's `validate_session_id`.
    ///
    /// `None` as well when the root itself is a link or a junction
    /// ([`AttachmentStore::new`]): the folder this would hand back is a folder in
    /// the tree the root names, and every caller here would then read it, write
    /// it or delete it as if it were the store's. One door closed is what keeps
    /// those three refusals instead of three holes, and an id that names nothing
    /// is already a state all three handle.
    ///
    /// The comment this replaces said that refusing `.` and `..` "removes the
    /// only way a session id could name a directory outside the store". That was
    /// false, and false in the direction that does damage: it was the reason
    /// nobody looked further. `Path::join` with a *rooted* path does not climb
    /// out of the base, it **replaces** it — `self.root.join("C:\Windows\Temp")`
    /// is `C:\Windows\Temp`, and an id like `\\server\share\x` names a share no
    /// root was ever part of. `.` and `..` were two spellings of the hole among
    /// many: `a/../..`, `C:x`, an NTFS stream (`file:stream`), the reserved
    /// device names (`CON`, `NUL`, `COM1`). A rule against a list of spells
    /// loses that argument eventually, so the rule is an alphabet instead, and
    /// everything outside it is `None`.
    pub(crate) fn session(&self, session_id: &str) -> Option<SessionAttachments> {
        if !self.available {
            return None;
        }
        if !is_session_folder_name(session_id) {
            return None;
        }
        Some(SessionAttachments {
            session_id: session_id.to_string(),
            dir: self.root.join(session_id),
            root: self.root.clone(),
            write_lock: Arc::clone(&self.write_lock),
        })
    }

    /// Drop one session's folder and report the bytes it held.
    ///
    /// The write lock is taken here for the same reason `materialize` takes it.
    /// The temp file plus rename protects a *reader* from observing half an
    /// image; it does not protect the *writer* whose folder is deleted between
    /// the temp write and the rename. Holding the lock across the removal makes
    /// a close wait for the write in flight instead of pulling the directory
    /// out from under it.
    ///
    /// # What the return value answers
    ///
    /// "How many bytes did this call take out of the store", which is what a
    /// caller releasing a reservation needs, and `None` when the store cannot
    /// answer that. It is deliberately *not* "did this session exist":
    ///
    /// - `Some(n)`: the folder is gone and held `n` bytes. A session that never
    ///   existed and a session whose folder was empty both answer `Some(0)`.
    ///   Those are two different questions, but the caller's next move is the
    ///   same one — release nothing — and a return of `Option<u64>` has no third
    ///   state to tell them apart with. A caller that genuinely needs the
    ///   distinction keeps its own record of what it opened, or asks
    ///   [`AttachmentStore::session_bytes`] before closing.
    /// - `None`: no number is safe to act on, so the caller keeps what it had
    ///   reserved. That is a folder the walk could not read, whose size was
    ///   never known; a removal that failed and left the files on disk; or an id
    ///   whose folder cannot be told from one in a root the walk never listed.
    ///
    /// It seeds the cache to answer, so the first close in a process pays the
    /// one walk every other budget question pays.
    pub(crate) fn remove_session(&self, session_id: &str) -> Option<u64> {
        // An id outside [`is_session_folder_name`] names no folder — `..`, a
        // path, a reserved device name — so this call drops nothing and the
        // caller releases nothing: the same answer as an id whose folder is not
        // there.
        let Some(session) = self.session(session_id) else {
            return Some(0);
        };
        // Poisoning is ignored the way `materialize` ignores it: a panic in some
        // unrelated thread must not make session close start failing.
        let mut state = self
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // The answer is what the folder held, so the picture has to be built
        // while the folder is still on the disk: a close is the first budget
        // question in many processes, and an empty map would answer zero for a
        // folder full of images.
        AttachmentStore::seed_locked(&self.root, &mut state);
        let held = held_bytes(&state, &session.session_id);
        // Scratch left by a run that died between its temp file and the rename
        // is bytes the walk counts and no `resolve` can ever name, so the total
        // must not keep them ([`discard_scratch`]). The folder is deleted on the
        // next line and takes them with it, so this is *not* what reclaims them:
        // it is what keeps a close whose `remove_dir_all` fails — one file still
        // open, a mapped image on Windows — from leaving the residue to be
        // counted again by the next process. Safe under this guard for the same
        // reason the removal below needs it: no write into this folder can be
        // running while the lock is held.
        discard_scratch(&session.dir);
        let removed = std::fs::remove_dir_all(&session.dir);
        if removed.is_err() && session.dir.exists() {
            // The files are still on disk, so the store still holds them and no
            // number is safe to hand back. The entry stays for the same reason:
            // over-counting refuses, under-counting admits.
            return None;
        }
        // The folder's whole contribution leaves with it, which is what the
        // per-session key is for: nothing has to know which digests were in
        // there.
        state.sessions.remove(&session.session_id);
        held
    }

    /// Delete every session folder whose newest write is older than `max_age`,
    /// and report what each removal took out of the store.
    ///
    /// `now` is a parameter so the retention rule can be tested without moving
    /// real file timestamps around.
    ///
    /// A store whose root is itself a link or a junction sweeps nothing and
    /// reports nothing: a `remove_dir_all` reached through the root deletes in
    /// the tree the root names, and no folder under it is this store's
    /// ([`AttachmentStore::new`]).
    ///
    /// An entry that is a link is skipped for the same reason one level down.
    /// Measured rather than assumed, because the obvious reading of the line
    /// below is the wrong one: `DirEntry::metadata` does *not* follow the name —
    /// on Windows it answers from the listing, and the listing describes the
    /// reparse point, so a junction to a folder reports `is_dir == false` there
    /// just as a symlink does (and `lstat` answers the same on POSIX). The
    /// filter below therefore already passes a redirect over on today's
    /// toolchain, and this check says so outright instead of resting a deletion
    /// on a classification that `is_redirect` exists because it has changed
    /// across versions.
    ///
    /// # Why the shape is a list and not a total
    ///
    /// A caller keeping a counter per device has to subtract from the right one,
    /// and this store cannot do that attribution for it: a session id's middle
    /// segment is a connection token and not an identity (see the module
    /// header), and a folder is otherwise just a name on a disk. So the sweep
    /// hands back one entry per removal — `(session id, bytes reclaimed)` — and
    /// the caller, which is the only party that knows which sessions belong to
    /// which device, does the summing. A bare total would be unusable for that
    /// caller; a count alone is what this used to return while a counter
    /// elsewhere went on counting bytes the store had already deleted.
    ///
    /// The count is still here, as [`Vec::len`]: a folder that was not a
    /// candidate, one whose removal failed, and one whose name is not a string
    /// (never keyed, never counted toward the total) are all simply not in the
    /// list. `None` for the bytes is a folder whose size the store never knew
    /// ([`SessionBytes::Unknown`]), which a caller must keep counted rather than
    /// release. Entries appear in the order the filesystem listed the folders.
    pub(crate) fn sweep_older_than(
        &self,
        now: SystemTime,
        max_age: Duration,
    ) -> Vec<(String, Option<u64>)> {
        if !self.available {
            return Vec::new();
        }
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            // No store yet is the normal state of a fresh install.
            return Vec::new();
        };
        let mut reclaimed: Vec<(String, Option<u64>)> = Vec::new();
        for entry in entries.flatten() {
            // Before the metadata read and before the `is_dir` filter, which is
            // where `seed_locked` asks the same question: a redirect is skipped
            // by that filter today (see the doc comment above), and this check is
            // what keeps it skipped rather than what first notices it.
            if is_redirect(&entry.path()) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_dir() {
                continue;
            }
            // A cheap filter, taken without the lock, so the lock stays off the
            // folders that obviously are not candidates. The decision is made
            // again under the lock by `remove_if_still_older_than`.
            if is_older_than(&entry.path(), now, max_age) != Some(true) {
                continue;
            }
            if let Some(removed) = self.remove_if_still_older_than(&entry.path(), now, max_age) {
                reclaimed.push(removed);
            }
        }
        reclaimed
    }

    /// Remove one candidate folder, but only if it is still older than
    /// `max_age` with the write lock held. Returns the session it removed and
    /// the bytes that folder held, and `None` when it removed nothing.
    ///
    /// The sweep's age filter runs without the lock, so its answer can be out
    /// of date by the time the lock is held: a write into the folder can land
    /// in between, and deleting then would throw away an attachment the user
    /// made moments ago. Under the lock the newest write cannot change, so the
    /// age is read again here and a folder that is no longer older than the
    /// limit is kept.
    ///
    /// The lock is taken per removal, not around the whole walk. The walk reads
    /// every folder and can run long, and one process-wide mutex held across it
    /// would park every materialize behind a directory scan; the race this
    /// closes is between one delete and one write into the folder being
    /// deleted, so the lock only has to cover the delete.
    ///
    /// `None` for the bytes of a removal is a folder the walk could not read,
    /// and it is not the same as failing to remove one: a removal that fails
    /// returns `None` for the whole call, so a caller cannot release a
    /// reservation for files that are still on the disk.
    fn remove_if_still_older_than(
        &self,
        dir: &Path,
        now: SystemTime,
        max_age: Duration,
    ) -> Option<(String, Option<u64>)> {
        // Poisoning is ignored the way `materialize` ignores it: a panic in
        // some unrelated thread must not make retention start failing.
        let mut state = self
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if is_older_than(dir, now, max_age) != Some(true) {
            return None;
        }
        // The folder's own name is the key. A name that is not a string was never
        // keyed and never counted — this store builds every folder here from a
        // `&str` — so such a folder is removed like any other and reported as
        // nothing: there is no total it was ever part of.
        let key = dir
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string);
        // Read before the removal, because the removal takes the entry with it.
        let held = key.as_deref().map(|name| held_bytes(&state, name));
        if std::fs::remove_dir_all(dir).is_err() {
            return None;
        }
        let session_id = key?;
        state.sessions.remove(&session_id);
        Some((session_id, held.flatten()))
    }
}

/// The deposit half: the store's budget question, the per-session question, the
/// write that answers them, and the lookup that hands a stored path back.
///
/// Nothing in the daemon calls any of this yet. The caller is the
/// `SessionDeposit` arm in `session.rs`, and the decision above these functions
/// — whether this connection may write into this session at all — belongs to
/// that file (`check_user_owner`). A build without tests would report the whole
/// block as unused until the arm lands, which is why it carries the marker
/// `idempotency`'s store carries for the same reason; the marker comes off when
/// the arm that calls this exists.
#[cfg_attr(not(test), allow(dead_code))]
impl AttachmentStore {
    /// Store one attachment for one session and report what was written.
    ///
    /// # What this assumes, and does not check
    ///
    /// That the session may be written to at all. Deciding that needs the
    /// session registry and the connection's identity, and the store has
    /// neither: `session.rs` asks `check_user_owner` before it gets here, and
    /// calling this without that decision is a bug in the caller rather than
    /// something this function could notice. What it does enforce on its own is
    /// the shape of the id — it has to name a folder inside the store — and the
    /// store's byte budget. Nothing here reads either segment of the id: the
    /// budget is the store's own (see the module header), so no part of a
    /// session id is a key.
    ///
    /// # A refusal leaves nothing behind
    ///
    /// The budget is checked before the file is created, against the bytes that
    /// will be written (decoded and stripped), never against a number the
    /// caller supplied. `stored_bytes` in the reply is the size of the file on
    /// disk, because the file is the only party that knows how big a stored
    /// attachment is.
    ///
    /// A budget that cannot be *computed* is a refusal as well, and not a number
    /// rounded down: when one folder could not be read the total is unknown, and
    /// the honest answer is to refuse rather than guess (`store_total`).
    ///
    /// Depositing bytes this session already holds is accepted and adds
    /// nothing: that deposit creates no file, so it holds no bytes and asks no
    /// budget question. The already-stored check is therefore the first thing the
    /// guard does ([`already_stored`], and only a regular file answers yes), and
    /// neither the limit nor an unknown total can refuse a file the session
    /// already has. The one refusal that comes before it is the id's: an id that
    /// names no folder is refused whether or not those bytes are already stored.
    pub(crate) fn deposit(
        &self,
        session_id: &str,
        attachment: &PromptAttachment,
    ) -> Result<Deposited, WireError> {
        let Some(session) = self.session(session_id) else {
            return Err(no_such_session());
        };
        // Decoding and walking an image is the expensive half of a deposit and
        // depends on nothing the lock protects, so it runs outside it — the
        // shape `materialize` already had.
        let (extension, stored) = prepare(attachment)?;
        let digest = sha256_hex(&stored);
        let path = session.dir.join(format!("{digest}.{extension}"));

        let mut state = self
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // The seed, the exists check, the budget check and the write are
        // [`admit_locked`]'s, and they are one function because they are one
        // order: the inline path used to run its own copy of this sequence with
        // the budget check missing, which is the whole of what that path got
        // wrong. One copy of the order cannot drift from itself again.
        admit_locked(&self.root, &mut state, &session.session_id, &path, &stored)?;
        Ok(Deposited {
            digest,
            stored_bytes: stored_size(&path)?,
            path,
        })
    }

    /// The path and the real size of one stored attachment.
    ///
    /// The digest names a file inside one session's folder and nowhere else: a
    /// digest resolves only in the session it was deposited to. That is the
    /// rule the wire states for references, and it is why this takes a session
    /// id at all. The name is constructed from the validated digest and the
    /// store's own extension table, never accepted as a path, so there is no
    /// spelling of a digest that could reach outside the folder.
    ///
    /// `extension_hint` is a shortcut and not the answer. A reference carries a
    /// session, a digest and a size, and no MIME type, so a caller often knows
    /// nothing that would name the file; when it does know, passing it here
    /// saves a `read_dir`, and when the hint is absent or wrong the folder's
    /// listing answers instead. Only the three extensions this store writes are
    /// ever built from a hint, so a hint cannot introduce a name the store did
    /// not write.
    ///
    /// The size is the file's own, read here. D2's rule is that a stored
    /// attachment's size is the file's to state: the caller compares this
    /// against a reference's `stored_bytes` and refuses a disagreement, and it
    /// cannot compare against a number this function remembered instead of
    /// read. A file that is not there is a refusal, not a size of zero.
    ///
    /// No lock is taken. The write this could race renames the file into place,
    /// so a reader sees the whole file or no file and never half of one;
    /// waiting on the store's write lock would park every reference behind
    /// every deposit to close a window that does not exist. The file can still
    /// be removed after this returns — the caller is about to hand the path to
    /// a provider — and no lock this function could take would outlive that.
    pub(crate) fn resolve(
        &self,
        session_id: &str,
        digest: &str,
        extension_hint: Option<&str>,
    ) -> Result<(PathBuf, u64), WireError> {
        if !is_digest(digest) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                invalid_attachment_digest_message(),
            ));
        }
        let Some(session) = self.session(session_id) else {
            return Err(no_such_session());
        };
        // The folder before the file: `find_stored` refuses an entry that is a
        // link, and a session folder that is one is the same refusal one level
        // up — a listing through it hands a provider a path outside the store,
        // and this call is the read side of the hole `refuse_redirect` guards
        // ([`AttachmentStore::new`] refuses the root, this refuses the folder).
        refuse_redirect(&session.dir, "the session folder")?;
        let Some(path) = find_stored(&session.dir, digest, extension_hint) else {
            return Err(no_stored_file());
        };
        let stored_bytes = stored_size(&path)?;
        Ok((path, stored_bytes))
    }

    /// Every byte this store holds, or `None` when they cannot be counted.
    ///
    /// The budget question, for a caller that has to ask it outside a write: a
    /// prompt's references are checked before a provider is asked to read
    /// anything, and the answer has to be the store's rather than the client's.
    /// The first call in a process is the walk; every later one is a sum over
    /// one map, because the answer *is* that sum ([`store_total`]) — there is no
    /// key to fold it by and none to be wrong about.
    ///
    /// `None` is not zero and must not be rendered as one: it means a folder
    /// could not be listed, so the store's bytes cannot be counted at all. A
    /// caller with a budget to enforce refuses on `None` the way `deposit`
    /// does; a caller showing a number has to say it does not know. A root that
    /// is itself a link or a junction is `None` before the walk starts, for the
    /// same reason: the folders a walk would find are in the tree the root names,
    /// and what they hold is not this store's to count
    /// ([`AttachmentStore::new`]).
    ///
    /// It takes the store's write lock, and `std::sync::Mutex` is not
    /// reentrant. Calling it from inside a guarded section is a hang, which is
    /// why the deposit path does not call it: `deposit` asks `store_total` on
    /// the guard it already holds, because its check and the write it gates have
    /// to be one critical section. The two are the same sum.
    pub(crate) fn store_bytes(&self) -> Option<u64> {
        if !self.available {
            return None;
        }
        let mut state = self
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        AttachmentStore::seed_locked(&self.root, &mut state);
        store_total(&state)
    }

    /// How many bytes one session holds, or `None` when that cannot be counted.
    ///
    /// The per-session half of the budget, for a caller that has to attribute
    /// bytes to something smaller than the store — the peer gate reserving
    /// against a device, which is the only party that knows which sessions
    /// belong to it. This store cannot answer that question and does not pretend
    /// to: a session id's middle segment is a connection token and not an
    /// identity (see the module header), and a folder is otherwise just a name.
    ///
    /// `None` is not zero, and it is the same silences the total has: the
    /// session is one the walk could not read, or the root itself could not be
    /// listed. An id with no folder behind it is `Some(0)` — it holds nothing —
    /// and an id the store will not turn into a folder at all (`.` or `..`, or an
    /// id this store refuses as a name, or any id at all when the root is a link)
    /// is `None`, because there is no session to report on rather than an empty
    /// one. The last of those is the store's own refusal read back: `session`
    /// answers `None` for a root that is not its own, and a session this store
    /// cannot reach is not a session holding zero bytes.
    ///
    /// Same lock and the same warning as [`AttachmentStore::store_bytes`]: a
    /// `std::sync::Mutex` is not reentrant, and this takes the guard.
    pub(crate) fn session_bytes(&self, session_id: &str) -> Option<u64> {
        let session = self.session(session_id)?;
        let mut state = self
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        AttachmentStore::seed_locked(&self.root, &mut state);
        held_bytes(&state, &session.session_id)
    }

    /// Build [`StoreState::sessions`] from the tree, once a complete walk has
    /// been made.
    ///
    /// Called with the guard held, so two threads cannot walk at once and the
    /// second caller finds the flag set rather than a second walk. The walk is
    /// one listing of the store root and one per session folder — the cost D4
    /// accepts as one-time in exchange for never paying it per deposit.
    ///
    /// It rebuilds rather than adds. A write that landed before the first
    /// budget question has already charged itself to a map that had not been
    /// built yet, and its file is on disk for this walk to find; keeping the old
    /// map would count that file twice. Rebuilding is also what makes the tree
    /// the authority over what the cache believes: a folder this process did not
    /// create, and a file a crash left behind, are counted by what is there now.
    ///
    /// A folder that could not be read is recorded as [`SessionBytes::Unknown`]
    /// and not as zero, and a walk that recorded one does not set `seeded`: the
    /// flag means "this map is the whole picture", and a walk that had to guess
    /// about a folder is not one. The next budget question walks again, so a
    /// transient failure heals itself instead of one unreadable folder standing
    /// for the rest of the process. A folder that stays unreadable costs a walk
    /// per question, which is the price of not freezing a wrong number for a
    /// process lifetime.
    ///
    /// The root is a parameter rather than `self.root` because the walk has two
    /// callers that are not the store: [`admit_locked`], which the inline path
    /// reaches through a handle holding a clone of the write lock rather than an
    /// `AttachmentStore`. An associated function instead of a free one so the
    /// root and the rule about it stay one thing to read.
    fn seed_locked(root: &Path, state: &mut StoreState) {
        if state.seeded {
            return;
        }
        state.sessions.clear();
        state.root_unreadable = false;
        // Set below, and only by a walk that read everything it looked at.
        let mut complete = true;
        match std::fs::read_dir(root) {
            Ok(entries) => {
                for entry in entries {
                    // An entry that cannot be read makes the walk partial, and a
                    // partial walk is a picture of nothing: the folder it hides
                    // is a folder whose bytes would be missing from the total.
                    let Ok(entry) = entry else {
                        complete = false;
                        continue;
                    };
                    // The same skip the sweep makes, in the same place relative
                    // to the metadata read, and measured the same way: a
                    // redirect is not a folder this store holds, so its bytes are
                    // not this store's to count — a total that charged the tree
                    // behind a junction is the number the limit is enforced
                    // against. On today's toolchain the filter below already
                    // passes one over (`entry.metadata` answers for the reparse
                    // point), so this is a rule stated outright rather than a
                    // miscount repaired; see the sweep's doc comment for the
                    // measurement and for the version that says otherwise.
                    if is_redirect(&entry.path()) {
                        continue;
                    }
                    let Ok(metadata) = entry.metadata() else {
                        complete = false;
                        continue;
                    };
                    if !metadata.is_dir() {
                        continue;
                    }
                    // A folder whose name is not a string cannot be keyed, and
                    // no session id is one. What such a folder holds is not
                    // guessed at either: it is left out of the total rather than
                    // counted under a name nothing could look up, and that is
                    // why it does not make the walk partial — there is no total
                    // it could be wrong about.
                    let Ok(session_id) = entry.file_name().into_string() else {
                        continue;
                    };
                    let bytes = match folder_bytes(&entry.path()) {
                        Some(bytes) => SessionBytes::Known(bytes),
                        None => {
                            complete = false;
                            SessionBytes::Unknown
                        }
                    };
                    state.sessions.insert(session_id, bytes);
                }
            }
            // Nothing to list is a store with nothing in it: a real zero, and
            // the normal state of a fresh install. A root that is *there* and
            // unlistable is the other case, and it is not zero — it is the whole
            // total going unknowable at once.
            Err(_) if !root.exists() => {}
            Err(_) => {
                state.root_unreadable = true;
                complete = false;
            }
        }
        if complete {
            state.seeded = true;
        }
    }
}

/// What one accepted deposit produced.
///
/// The three things a deposit reply is built from: the digest that names the
/// file (a client copies it into a reference rather than deriving one — the name
/// is the digest of the *stored* bytes, which the strip made different from what
/// was sent), the size the store actually wrote, and the path those bytes live
/// at.
///
/// No field is read outside the tests yet. The deposit reply is built by the
/// `SessionDeposit` arm in `session.rs` (`session_deposit`, which has landed)
/// — the same reason the block that returns this carries a dead-code marker.
/// `Debug` is derived rather than written by hand, unlike `PromptAttachment`'s:
/// nothing here is the attachment's content. A digest, a byte count and a path
/// are the identifiers this store already puts in its own error text, so a
/// derive cannot spill bytes the way printing `data` would.
#[derive(Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct Deposited {
    /// SHA-256 of the bytes that were written, lowercase hex.
    pub(crate) digest: String,
    /// The size of the file on disk, not of the request.
    pub(crate) stored_bytes: u64,
    /// Where the bytes are: inside the session's own folder.
    pub(crate) path: PathBuf,
}

/// One session's attachment folder.
pub(crate) struct SessionAttachments {
    /// The id this folder is named after, kept as a string because the budget
    /// cache is keyed by it: `dir`'s file name would have to be read back out,
    /// and a folder whose name is not UTF-8 would then be charged to nothing at
    /// all instead of to its session.
    session_id: String,
    dir: PathBuf,
    /// The store's root, kept beside `dir` because the write path has to make
    /// the root private and refuse a root that is a redirect, and this struct
    /// has no way back to the `AttachmentStore` that made it: the lock it holds
    /// is a clone of the store's, not the store.
    root: PathBuf,
    write_lock: Arc<Mutex<StoreState>>,
}

impl SessionAttachments {
    /// Write one attachment to disk and return its absolute path.
    ///
    /// Re-materializing the same bytes returns the existing path and does not
    /// rewrite the file.
    ///
    /// A raster is walked and its identity metadata taken out
    /// ([`crate::raster_metadata`]) between the decode and the digest, so the
    /// bytes written are not always the bytes the client sent. The name is
    /// built from the bytes that were written, which makes the consequence
    /// worth stating plainly, because a content-addressed name reads like a
    /// promise about the input: **the file is named after the stripped bytes.**
    /// A client that predicts the attachment path from the digest of the bytes
    /// it sent is therefore wrong whenever anything was stripped. That is
    /// intended, not a defect. The daemon is not transparent about
    /// attachments; it rewrites them before forwarding, and the path is the
    /// digest of what is actually on disk.
    ///
    /// A raster this pass cannot walk is refused rather than stored. Handing
    /// the input back on a parse failure would re-admit exactly the bytes the
    /// caller asked to have removed and would report a strip that did nothing
    /// as a success.
    ///
    /// SVG is deliberately not stripped here, and that is a decision rather
    /// than an oversight. The frontend sanitises SVG source
    /// (`sanitizeSvgSource`) and this side does not, so an SVG arriving from
    /// another device — the very path this pass exists for — is written
    /// unsanitised. It is a known, named gap: the raster rule is a byte-level
    /// walk and the SVG rule is a source-level parse with its own vocabulary,
    /// and this pass does not guess at the second one. Closing it means
    /// porting that sanitiser, not extending this one.
    ///
    /// Which container the bytes are is decided by the bytes, not by the
    /// `mime_type` the client sent. The declared type is the sender's word and
    /// the bytes are the evidence, and the two have to agree: the strip used to
    /// be selected from the label alone, so a JPEG full of coordinates labelled
    /// `image/svg+xml` took the write-through path and reached the provider
    /// untouched. The label is a field the sender controls, and it must not be
    /// the thing that decides whether the guarantee runs.
    ///
    /// So a file whose bytes and label disagree about the container is refused,
    /// and so is a file declared a raster that does not open with that
    /// container's signature — the rule of the pass is that what cannot be
    /// walked is refused rather than handed back intact. The label still decides
    /// the *extension*, which is part of why a disagreement is a refusal and not
    /// a correction: there is no extension to write that would be honest about
    /// both what was declared and what the bytes are.
    pub(crate) fn materialize(&self, attachment: &PromptAttachment) -> Result<PathBuf, WireError> {
        // The decode, the agreement check and the strip are `prepare`'s, which
        // `deposit` runs too: one copy of the rules, so the two paths cannot
        // disagree about what may be stored.
        let (extension, stored) = prepare(attachment)?;
        let path = self
            .dir
            .join(format!("{}.{extension}", sha256_hex(&stored)));
        // Poisoning is ignored the way every other lock in this file ignores it:
        // a panic in some unrelated thread must not turn every later attachment
        // into a refusal.
        let mut guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // The same call, and so the same sequence, as `deposit`: seed, recognise
        // bytes already held, ask the budget, write. What this path used to do
        // was charge the total without ever asking it, so an inline attachment —
        // the `SessionSend` path — could be the bytes that took the store past
        // `MAX_ATTACHMENT_OWNER_BYTES`: a limit the deposit path enforces and
        // this one silently spent. The budget is the store's own and is not
        // keyed (module header), so there is nothing to pass for it.
        admit_locked(&self.root, &mut guard, &self.session_id, &path, &stored)?;
        Ok(path)
    }
}

/// Delete the scratch this store's own writer leaves behind, and report the bytes
/// that leave with it.
///
/// `atomic_write` stages `<digest>.tmp` beside the target and copies a
/// `<digest>.bak` when one is already there, and a run that died between the
/// staging and the rename leaves one of those behind. Neither name is one
/// `find_stored` can return — it takes a stored extension, and `tmp`/`bak` are
/// not in `STORED_EXTENSIONS` — so such a file is bytes every walk counts and no
/// `resolve` can ever hand back: budget spent for the life of the folder on
/// something no caller can name.
///
/// Safe against a write that is in flight, and that is what the lock is for:
/// every caller holds the store's write lock, `atomic_write`'s temp exists only
/// inside the critical section that created it, and its name is derived from the
/// digest that same critical section computed. So a `.tmp` seen from inside the
/// lock belongs to no write that is still running — and only one write runs at a
/// time (the struct comment on [`AttachmentStore`]).
///
/// The size is counted the way [`folder_bytes`] counts it and only when the
/// removal succeeded: a file that is still there is still charged, so no caller
/// can subtract bytes the tree is still holding.
fn discard_scratch(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut reclaimed: u64 = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !(name.ends_with(".tmp") || name.ends_with(".bak")) {
            continue;
        }
        let bytes = match entry.metadata() {
            Ok(metadata) if metadata.is_file() => metadata.len(),
            _ => 0,
        };
        // `remove_file` removes the name and not whatever a link at it pointed
        // at, which is the behaviour this wants: a symlink planted at a temp
        // name is exactly the thing that must not survive to be written through.
        if std::fs::remove_file(entry.path()).is_ok() {
            reclaimed = reclaimed.saturating_add(bytes);
        }
    }
    reclaimed
}

// One folder name a test has asked `harden` to fail on.
//
// A thread-local rather than a field of the store, because the walk runs in
// `AttachmentStore::new`: the failure has to be in place *before* the store
// exists, and the store a test then inspects has to be the one that open
// produced. Thread-local because the test harness runs each test on its own
// thread, so one test's failure cannot reach a store another test opens. A `//`
// comment and not a doc one: `thread_local!` is a macro, and a `///` above it
// documents nothing.
#[cfg(test)]
thread_local! {
    static HARDEN_FAILURE: std::cell::RefCell<Option<std::ffi::OsString>> =
        const { std::cell::RefCell::new(None) };
}

/// Give one folder the current user's DACL, with a seam a test can pull.
///
/// The seam is here and not inside [`restrict_to_current_user`] because what
/// DEP-17 decides is what the walk *does* with a failure, and that decision has
/// to be reachable on a platform where the call underneath is a no-op: a test
/// that could only fail a real DACL write would not run on POSIX at all, and the
/// failure path would then be the one thing nobody exercises.
fn harden(path: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(blocked) = HARDEN_FAILURE.with(|name| name.borrow().clone()) {
        if path.file_name() == Some(blocked.as_os_str()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "a test asked this folder not to be hardened",
            ));
        }
    }
    restrict_to_current_user(path)
}

/// [`harden`] one folder and say whether it worked, printing the one that did
/// not.
///
/// Printed rather than returned: the refusal a caller sees is the store being
/// unavailable, and this line is what says which folder is behind it. Silence is
/// the finding this answers — a store that opens with a folder nobody narrowed
/// leaves nothing to look at afterwards.
fn harden_or_report(path: &Path) -> bool {
    match harden(path) {
        Ok(()) => true,
        Err(error) => {
            eprintln!(
                "could not narrow the attachment folder {} to the current user: {error}",
                path.display()
            );
            false
        }
    }
}

/// Give every folder under a root the current user's DACL, and delete the
/// scratch this store's own writer leaves behind. Report whether every DACL it
/// meant to set was set.
///
/// Two duties in one walk because both are about a folder this store did not
/// write into yet, and both are cheap to do once at the open:
///
/// *The DACL.* [`restrict_to_current_user`] otherwise runs only in
/// [`prepare_session_dir`], which is the first *write*, and `resolve` reads a
/// folder before any write has been through it: a folder an earlier build left
/// behind keeps whatever its parent granted — on a default Windows profile that
/// includes accounts that are not this user. Applying it is idempotent, so a
/// folder this store later writes into is narrowed twice and says the same
/// thing.
///
/// *The scratch.* See [`discard_scratch`].
///
/// A failure to narrow a folder is neither silent nor best effort. The folder
/// keeps the DACL its parent granted — on a default Windows profile one that
/// includes other accounts — and `resolve` would read attachments out of it
/// afterwards, so the walk answers `false` and [`AttachmentStore::new`] makes the
/// store unavailable for it ([`harden_or_report`] prints the folder and the
/// error). The scratch half stays best effort for the reason it always was: it
/// removes this store's own leftovers, and a leftover that survives costs budget
/// for one folder's lifetime rather than secrecy.
///
/// A root that is *not there*, or that is *not a folder*, is nothing to harden:
/// a fresh install has to open, and a file where the root belongs has no folders
/// under it to narrow — the budget's own reading of that root answers for it, so
/// a deposit there is still refused, and with the cause that says so. A root that
/// is a folder and *cannot be listed* **is** a failure, and it is the one
/// judgement call here: the walk cannot see the folders under it, so it cannot
/// narrow the DACL of a folder `resolve` would then read, and a refusal to open is
/// the only answer that cannot be wrong about a folder the store never looked at.
///
/// Folders that are redirects are skipped — a junction in this root names
/// somebody else's tree, and this store does not delete files in one nor set a
/// DACL on one — and so is a root that is itself a redirect, which
/// [`AttachmentStore::new`] has already answered for every other call. A redirect
/// root is not a failure *here*: this walk's answer is about the DACLs it meant
/// to set, and `new` refuses that root on its own ground.
fn harden_and_sweep_in_store(root: &Path) -> bool {
    if is_redirect(root) {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        // Nothing to harden is not a failure — see the doc comment: a root that
        // is not there is a fresh install, and a root that is not a folder has no
        // folders under it. A root that is a folder and cannot be listed is the
        // refusal, because the folders under it are folders this walk never saw.
        return !(root.exists() && root.is_dir());
    };
    // A listing that succeeded is a root that is there and is a folder, so the
    // root's own DACL is applied exactly when the root exists.
    let mut hardened = harden_or_report(root);
    for entry in entries.flatten() {
        let path = entry.path();
        if is_redirect(&path) {
            continue;
        }
        if entry.metadata().is_ok_and(|metadata| metadata.is_dir()) {
            hardened &= harden_or_report(&path);
            discard_scratch(&path);
        }
    }
    hardened
}

/// Refuse a path when the name is a link rather than a folder or file.
///
/// Windows first, because that is where the damage is concrete: a junction at
/// the session folder's name turns a deposit into a write into another tree and
/// `remove_session` into a delete of one, and `fs::write` of the temp file
/// follows a symlink planted at the temp's name. A POSIX symlink is the same
/// hole with a different spelling, so the check is not `#[cfg]`-ed.
///
/// The read paths are the same hole with the arrow the other way, which is why
/// the refusal is worded for both: a listing through a junction is how a
/// `resolve` hands a provider a path outside the store, and a link at a digest's
/// name is how it hands back a file the store never wrote.
///
/// `symlink_metadata` and not `metadata`: the question is what the name is, not
/// what it points at, and a dangling link is still a link. The refusal names the
/// path and what is wrong with it, because "io error" is not something a caller
/// reading the log can act on.
fn refuse_redirect(path: &Path, what: &str) -> Result<(), WireError> {
    if !is_redirect(path) {
        return Ok(());
    }
    Err(WireError::new(
        ErrorCode::Io,
        format!(
            "Refusing to reach an attachment through a link or junction: {what} at {} is a reparse point.",
            path.display()
        ),
    ))
}

/// Whether `path` is a symbolic link, a junction, or another redirecting reparse
/// point.
///
/// A path that is not there is not a redirect: the caller's next move is to
/// create it, and creating a name is not following one.
///
/// A junction (`mklink /J`, the spelling an unprivileged process can create on
/// Windows) is asked about by attribute rather than through
/// `FileType::is_symlink`: whether the standard library classifies a mount point
/// as a symlink has changed across versions, and this answer must not depend on
/// the toolchain. The attribute is also the one test that covers a reparse tag
/// Rust has no name for.
fn is_redirect(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_REPARSE_POINT: `windows_sys` is a dependency of this
        // crate, but the constant has no other user here and one literal with
        // this comment is cheaper than a second import path to keep in step.
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// Whether `path` names a regular file of this store's, which is a file and not
/// a link to one.
///
/// [`Path::is_file`] is not this question: it follows the name, so a symlink
/// planted at a digest's name answers for the file it points at — anywhere on
/// the machine — and a listing that used it would hand a provider a path outside
/// the store. The name has to be a stored file itself, so the answer is
/// [`symlink_metadata`](std::fs::symlink_metadata) and not `metadata`, with
/// [`is_redirect`] covering the reparse tag `FileType` does not name.
fn is_regular_file(path: &Path) -> bool {
    !is_redirect(path) && std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

/// Create the two folders a write needs, and make them private.
///
/// The order is the point: the root first, then its DACL, then the session
/// folder, then its DACL. A folder this store writes into is never left
/// inheriting whatever its parent granted — which on a default Windows profile
/// includes other accounts on the machine — and the DACL lands before the first
/// byte, so no attachment is ever on disk under a weaker one than it will carry.
/// `security.rs` owns the call so this file does not invent a second spelling of
/// "the current user only"; it is the same one `tool_policy.rs` and
/// `mcp_broker.rs` use.
///
/// The DACL is applied whether or not this call created the folder. It replaces
/// the DACL rather than merging into it (see `security.rs`), so it is idempotent,
/// and a folder that predates this rule is repaired at the store's open
/// (`harden_and_sweep_in_store`) and, for a folder that appears after the open,
/// by the next write into it instead of staying weak for the rest of its life.
///
/// Both folders are checked for a redirect *before* anything is created: a
/// junction at the session folder's name is a store that would write into
/// somebody else's tree, and creating "through" it is exactly what must not
/// happen. What this does not defend against is the same user planting the
/// junction between the check and the write — one account's process can always
/// race itself — and that is what the DACL is for, against the other accounts.
fn prepare_session_dir(root: &Path, dir: &Path) -> Result<(), WireError> {
    refuse_redirect(root, "the store root")?;
    refuse_redirect(dir, "the session folder")?;
    for folder in [root, dir] {
        std::fs::create_dir_all(folder)
            .and_then(|()| restrict_to_current_user(folder))
            .map_err(|error| {
                WireError::new(
                    ErrorCode::Io,
                    format!(
                        "Could not prepare the attachment folder {}: {error}",
                        folder.display()
                    ),
                )
            })?;
    }
    Ok(())
}

/// Give one path the daemon user's DACL, on Windows, through `security.rs`.
///
/// Windows only, and not as a shortcut: there is no DACL to set anywhere else.
/// The branch that does nothing is spelled out rather than left out so that the
/// other platform's behaviour is a decision on the page — and so that a build
/// which compiles this module without the `server` feature (the feature
/// `security.rs` puts `apply_current_user_dacl` behind) does not fail to build
/// over a call it could not make. `lib.rs` compiles this module only under
/// `server` today, so that arm is for the build that removes that gate, and it
/// is a no-op for the same reason the POSIX one is: there is nothing to call.
#[cfg(all(windows, feature = "server"))]
fn restrict_to_current_user(path: &Path) -> std::io::Result<()> {
    crate::security::apply_current_user_dacl(path)
}

#[cfg(any(not(windows), not(feature = "server")))]
fn restrict_to_current_user(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Whether the store already holds exactly this file.
///
/// A name that is a link, a junction or a directory is not "already stored": it
/// is a refusal. `Path::exists` follows the name, so the check it replaces said
/// *success* to a junction planted at a digest's name and to a folder sitting
/// there — and `deposit` then reported a file somebody else wrote, sized by
/// [`stored_size`], as this session's stored attachment, while `write_locked`
/// charged nothing and wrote nothing. The two facts a stored file is have to
/// stay the same fact: the bytes at that name are the digest's bytes, and they
/// are the store's.
///
/// The redirect check is first, and `Ok(false)` is only ever reached for a name
/// that is not there: an entry that is there and is not a regular file is a
/// refusal rather than "nothing to do" — a deposit that treated it as stored
/// would report what is not, and one that treated it as absent would write over
/// a name something else owns.
fn already_stored(path: &Path) -> Result<bool, WireError> {
    refuse_redirect(path, "the file this write would create")?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(WireError::new(
            ErrorCode::Io,
            format!(
                "Could not store an attached file: {} is not a stored file.",
                path.display()
            ),
        )),
        Err(_) => Ok(false),
    }
}

/// The one sequence every write into this store runs, in the order it has to run
/// in.
///
/// `deposit` and the inline path (`materialize`) both end here, and that is the
/// point of the function rather than a tidiness: they used to run the sequence
/// separately, and the inline copy had lost its middle — it charged the bytes of
/// a write that nothing had checked against the limit, so the path a
/// `SessionSend` carrying an inline attachment takes could put the store past a
/// budget the other path respects. One copy of the order cannot drift again.
///
/// The order, and why each step is where it is:
///
/// 1. [`AttachmentStore::seed_locked`]: the total is a sum over the cache, and
///    the cache is built by one walk, so a budget question asked before the walk
///    is a question about nothing.
/// 2. The already-stored check ([`already_stored`]): bytes this session already
///    holds are one file and one contribution, so they are not a budget question
///    at all, and neither the limit nor an unknown total may refuse them. Before
///    the check, not after. Only a regular file answers yes — a name taken by a
///    folder or by a link is a refusal, not a store.
/// 3. [`store_total`]: a total that could not be computed is not a total of
///    zero, and a check against a number below the truth admits exactly the
///    bytes the limit exists to refuse.
/// 4. The comparison, against the total the store *would* hold.
/// 5. [`write_locked`]: the only thing that creates a file, and the only thing
///    that charges one.
///
/// Called with the write guard held, and it has to be the same guard across the
/// whole sequence: a check and the write it admits are one critical section, or
/// two writers each fit under the limit and together do not.
fn admit_locked(
    root: &Path,
    state: &mut StoreState,
    session_id: &str,
    path: &Path,
    stored: &[u8],
) -> Result<(), WireError> {
    AttachmentStore::seed_locked(root, state);
    if already_stored(path)? {
        // The bytes are already here: no file, no bytes, no budget question. A
        // store whose total cannot be computed still hands back a file the
        // session already holds.
        return Ok(());
    }
    let Some(held) = store_total(state) else {
        return Err(budget_unknown());
    };
    let after = held.saturating_add(stored.len() as u64);
    if after > MAX_ATTACHMENT_OWNER_BYTES as u64 {
        return Err(over_budget(after));
    }
    write_locked(root, state, session_id, path, stored)
}

/// The bytes an attachment is stored as, and the extension they are stored
/// under, once the decode and the strip have run.
///
/// The order is the one `materialize` has always used: decode, then check that
/// the label and the bytes agree about whether this is a raster, then strip.
/// `deposit` runs the same function, so a deposit cannot be the path that skips
/// one of those steps — a deposit that skipped the agreement check would be the
/// bypass, and one that skipped the strip would write the metadata the store
/// exists to remove.
///
/// The expensive half of a write, and deliberately outside the lock: it touches
/// no file and depends on nothing the guard protects.
fn prepare(attachment: &PromptAttachment) -> Result<(&'static str, Vec<u8>), WireError> {
    let bytes = decode(&attachment.data)?;
    let extension = extension_for(&attachment.mime_type)
        .ok_or_else(|| unsupported_type(&attachment.mime_type))?;
    let stored = match (
        sniff_raster_mime(&bytes),
        RasterMime::from_mime_type(&attachment.mime_type),
    ) {
        // The label and the bytes agree, so the walk runs on what is really
        // there.
        (Some(sniffed), Some(declared)) if sniffed == declared => {
            strip_raster_metadata(&bytes, sniffed)
                .map_err(unreadable_image)?
                .bytes
        }
        // Neither the bytes nor the label say raster: the SVG path, and every
        // other type the extension table accepts. Written as it arrived, which
        // is the gap `materialize` names and not a new one.
        (None, None) => bytes,
        // Everything else is a disagreement. Either the bytes are a raster the
        // label misnames — including naming it a type that is not a raster at
        // all, which is the bypass this arm closes — or the label says raster
        // and the bytes do not open with the signature they claim. Both are
        // refused, and the alternative to refusing is writing bytes through a
        // guarantee that never examined them.
        _ => return Err(container_disagrees()),
    };
    Ok((extension, stored))
}

/// Write `stored` at `path` unless it is already there, and charge the bytes to
/// the session that holds them.
///
/// Callers hold the store's write lock, and both halves belong in that critical
/// section: a file must not exist and be uncounted, and the count must not move
/// without the file.
///
/// The charge is made after `atomic_write` returns and never on the early
/// return. Depositing the same bytes twice into one session is one file and one
/// increment; a deposit that created no file holds no bytes and is not a budget
/// question at all. Charging at the top instead would double-count exactly the
/// replay the content-addressed name exists to make free, until the store was
/// refusing itself a file it already held.
///
/// Every write into a session folder comes through here — `deposit` and
/// `materialize` both — so a file written for the inline path is counted like
/// any other file in that folder.
///
/// Everything the write depends on is settled here, before a byte of it exists:
/// the folders are created and given the daemon user's DACL
/// ([`prepare_session_dir`]), neither the session folder nor the target is a link
/// or a junction ([`refuse_redirect`]), and the scratch a killed run left in the
/// folder is gone ([`discard_scratch`]). The link check is what keeps the file
/// the digest names and the bytes behind it the same thing: a write told to
/// follow a link would report a path this store does not hold, counted in a
/// folder that holds nothing.
///
/// A write into a folder the walk could not read leaves that session
/// [`SessionBytes::Unknown`]. The bytes this process wrote are known and the
/// rest of the folder is not, so a total counting only the former would be the
/// same under-count the unknown state exists to remove.
fn write_locked(
    root: &Path,
    state: &mut StoreState,
    session_id: &str,
    path: &Path,
    stored: &[u8],
) -> Result<(), WireError> {
    // The same check the depositor ran, and it cannot be skipped on any path
    // into this function: `materialize`'s inline path reaches here through
    // `admit_locked`, but this is the only thing that creates a file, so the
    // name is answered for here as well. A name that is a link or a folder is a
    // refusal ([`already_stored`]) and not "nothing to do".
    if already_stored(path)? {
        return Ok(());
    }
    let dir = path.parent().ok_or_else(|| {
        WireError::new(
            ErrorCode::Io,
            "Could not store an attached file: the path has no folder.".to_string(),
        )
    })?;
    // The folders, their DACL, and nothing on the way through that is a link:
    // see the doc comment above for why each of these is before the write.
    prepare_session_dir(root, dir)?;
    // The target as well as its folder. A link at the file's own name is a write
    // that lands wherever the link points, and the file this store would then
    // "hold" is not the file it reports — while the folder it is counted in
    // holds nothing at all. Refusing is the only answer that keeps those two
    // facts the same fact.
    refuse_redirect(path, "the file this write would create")?;
    let scrubbed = discard_scratch(dir);
    if scrubbed > 0 {
        // The bytes left the store, so they leave the total with them. Only a
        // `Known` count can be adjusted: `Unknown` is a folder the walk could
        // not read and stays unknown for the reason it was.
        if let Some(SessionBytes::Known(bytes)) = state.sessions.get_mut(session_id) {
            *bytes = bytes.saturating_sub(scrubbed);
        }
    }
    // A temp file plus a rename: the agent reads this path from another
    // process, and it must never observe a half-written image.
    crate::atomic::atomic_write(path, stored).map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not store an attached file: {error}"),
        )
    })?;
    let entry = state
        .sessions
        .entry(session_id.to_string())
        .or_insert(SessionBytes::Known(0));
    match entry {
        SessionBytes::Known(bytes) => {
            *bytes = bytes.saturating_add(stored.len() as u64);
        }
        // A folder the walk could not read stays unknown when a file lands in
        // it: the bytes this process wrote are known and the rest of the folder
        // is not, and a total counting only the former would be a number nobody
        // can justify.
        SessionBytes::Unknown => {}
    }
    Ok(())
}

/// The digest that stands for one attachment in an idempotency fingerprint.
///
/// The sha256 of the decoded bytes, not the bytes: the fingerprint lives in an
/// in-memory table for every key the daemon has seen and must not weigh as much
/// as the images. `data` that does not decode is hashed as text instead, which
/// keeps the value total; such a request is refused before it can ever be
/// remembered under a key, so the fallback only has to be deterministic.
pub(crate) fn attachment_digest(attachment: &PromptAttachment) -> String {
    match decode(&attachment.data) {
        Ok(bytes) => sha256_hex(&bytes),
        Err(_) => sha256_hex(attachment.data.as_bytes()),
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode(data: &str) -> Result<Vec<u8>, WireError> {
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| WireError::new(ErrorCode::InvalidRequest, invalid_base64_message()))
}

/// The refusal for a deposit whose budget cannot be counted.
///
/// A session folder could not be listed (`folder_bytes`), so the store's total
/// is unknown and there is no number to compare against the limit. The refusal
/// is store-wide because the budget is: one unreadable folder is enough that any
/// number the check used would sit below the truth, so refusing every deposit is
/// the honest answer to not knowing. What lifts it is the next walk, not a guess
/// in the meantime (`seed_locked`).
///
/// `Io` rather than `InvalidRequest`: the request is well formed, and it is the
/// same request the client will make again once the folder can be read. What
/// failed is the store reading its own tree.
fn budget_unknown() -> WireError {
    WireError::new(
        ErrorCode::Io,
        "Could not count this store's attachments: a session folder could not be read.",
    )
}

/// The refusal for an id the store will not turn into a folder.
///
/// The id is not echoed. It is what this refusal is about, it arrives from the
/// wire with nothing here bounding its length, and the caller already knows
/// which id it asked with. Everything outside [`is_session_folder_name`] reaches
/// this — `.`, `..`, an absolute path, a UNC path, a path with a separator or a
/// colon in it, a reserved device name — and none of those names a session, so
/// the sentence is the daemon's usual one for an id no session answers to.
fn no_such_session() -> WireError {
    WireError::new(ErrorCode::SessionNotFound, "No session with that id.")
}

/// The refusal for a digest with no file behind it in this session.
///
/// Not `SessionNotFound`: the session is named by whoever asked, and may well
/// exist; what is missing is the file. `InvalidRequest` says the request is the
/// thing that has to change, which is true of both ways to arrive here — a
/// reference left over from a session whose attachments were swept, and a
/// digest deposited in a different session — and the store cannot tell those
/// apart, nor would telling them apart change the answer.
fn no_stored_file() -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        "The store holds no attachment with that digest in this session.",
    )
}

/// The refusal for a deposit that would put the store past
/// [`MAX_ATTACHMENT_OWNER_BYTES`].
///
/// The number that travels is the total the store would hold afterwards, not
/// what this deposit weighs: the caller's next move is to send fewer bytes, and
/// how many to cut follows from where the total would have landed. The sentence
/// is the shape the wire uses for the declared sum it can refuse on its own, so
/// a client shows both budget refusals alike.
///
/// It says "this store" rather than "you": the budget is not per client, and no
/// client is its owner — there is no spender to name, which is the whole reason
/// the key was deleted (see the module header).
fn over_budget(after: u64) -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        format!(
            "This deposit would take the stored attachments in this store to {after} bytes; the limit is {MAX_ATTACHMENT_OWNER_BYTES}."
        ),
    )
}

/// Whether `digest` may be joined to a path: 64 lowercase hex characters, and
/// nothing else.
///
/// The wire states the same rule for a reference's digest, in the protocol's
/// `attachments`, and this is not that check twice by accident. The wire's
/// answer decides whether a frame is well formed; this one decides whether a
/// string is a file name, and the store must not depend on a caller having
/// asked. Uppercase, a separator, a `.`, a 63- or 65-character near miss and a
/// traversal are all refused before a name is built, because the alternative is
/// a name that points wherever the string says — and a store whose only guard
/// was the caller's validation has a traversal the day a caller forgets.
fn is_digest(digest: &str) -> bool {
    digest.len() == DIGEST_HEX_LEN
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn unsupported_type(mime_type: &str) -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        unsupported_attachment_type_message(mime_type),
    )
}

/// The refusal for a raster the walk could not follow.
///
/// The sentence the walk produced travels: it names the byte offset or the
/// structure that did not add up, which is what makes a refusal actionable, and
/// it is derived from the file's own bytes rather than from anything the user
/// wrote. It is not the whole story the daemon could tell — the reader here is
/// a client showing one line — so the framing is the daemon's and the detail is
/// the walk's.
fn unreadable_image(reason: RasterStripError) -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        format!("An attachment's image could not be read: {reason}"),
    )
}

/// The refusal for a file whose bytes and declared type disagree.
///
/// The declared type is not echoed back. It is the field this refusal is about,
/// it arrives from the wire with nothing bounding its length, and the sender
/// already knows what they sent — so repeating it would put a string the sender
/// chose into a sentence the daemon writes. What is left is the part that can be
/// acted on: the contents are not what they were declared to be. The walk's byte
/// offsets are for a file the client is not being asked to inspect, which is why
/// this reads as a sentence about the attachment rather than about its bytes.
fn container_disagrees() -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        "An attachment's contents do not match the type it was sent as.".to_string(),
    )
}

fn extension_for(mime_type: &str) -> Option<&'static str> {
    match mime_type {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/svg+xml" => Some("svg"),
        // The finish report's artifact (`S5` decision 10): a child's last
        // message, deposited as one markdown file for the creator.
        "text/markdown" => Some("md"),
        _ => None,
    }
}

/// The MIME type the store's own extension names, for a read answer.
///
/// The reverse of [`extension_for`], over the same table: the reply states
/// the type of the file it hands back, and that statement has to come from
/// the store rather than from whoever named the reference.
pub(crate) fn mime_type_for_extension(extension: &str) -> Option<&'static str> {
    match extension {
        "png" => Some("image/png"),
        "jpg" => Some("image/jpeg"),
        "svg" => Some("image/svg+xml"),
        "md" => Some("text/markdown"),
        _ => None,
    }
}

/// The extension a hint names, if it names one this store writes.
///
/// A hint arrives either as the MIME type a caller happens to know
/// (`image/png`) or as the bare extension itself (`png`), and both come out
/// here as the extension. Anything else is `None`, and the caller lists the
/// folder instead: the hint is a shortcut, so a wrong one costs a `read_dir`,
/// while there is no spelling of a hint that names a file this store did not
/// write.
fn hinted_extension(hint: &str) -> Option<&'static str> {
    extension_for(hint).or_else(|| {
        STORED_EXTENSIONS
            .into_iter()
            .find(|extension| *extension == hint)
    })
}

/// The file in one session folder that `digest` names, if it is there.
///
/// The name is built and never taken: the stem is a validated digest, and the
/// extension comes from the store's own table — or from `hint`, which is a type
/// and not a path. Only one of the three extensions this store writes is ever
/// joined to a digest, so a caller cannot widen the search by naming a file, and
/// a hint that names anything else is simply not a shortcut this function takes.
///
/// The listing is what makes the answer independent of the hint: a reference
/// carries a session, a digest and a size and no MIME type, so a caller often
/// has nothing to hint with. It is also what keeps `atomic_write`'s siblings out
/// of the answer — a replace stages `<digest>.tmp` beside the target and copies
/// the old bytes to `<digest>.bak`, and both have the digest as their stem, so
/// the extension is not decoration: it is what makes those files not the file a
/// digest names.
fn find_stored(dir: &Path, digest: &str, hint: Option<&str>) -> Option<PathBuf> {
    if let Some(extension) = hint.and_then(hinted_extension) {
        let path = dir.join(format!("{digest}.{extension}"));
        // `is_regular_file` and not `is_file`: a link planted at the digest's
        // name follows to a file anywhere on the machine, and this function's
        // answer is a path a provider is asked to read ([`is_regular_file`]).
        if is_regular_file(&path) {
            return Some(path);
        }
    }
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let names_the_digest = path.file_stem().and_then(|stem| stem.to_str()) == Some(digest);
        let names_a_stored_file = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| STORED_EXTENSIONS.contains(&extension));
        if names_the_digest && names_a_stored_file && is_regular_file(&path) {
            return Some(path);
        }
    }
    None
}

/// The size of a stored file, read from the filesystem rather than remembered.
///
/// Every stored-byte number this file reports comes from here, so the size a
/// deposit replies with and the size a resolve compares are read the same way —
/// from the file. A remembered size would be a second copy of the truth, and the
/// one thing D2 refuses is a copy compared against itself.
///
/// A failure is an `Io` refusal and not a zero: a file whose size cannot be read
/// is not an empty file, and reporting zero would present a lock, a permission,
/// or a file that vanished mid-flight as a stored attachment of no bytes.
fn stored_size(path: &Path) -> Result<u64, WireError> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) => Err(WireError::new(
            ErrorCode::Io,
            format!("Could not read a stored attachment's size: {error}"),
        )),
    }
}

/// Every byte the store holds, summed over the sessions the cache knows, or
/// `None` when any of them cannot be counted.
///
/// A sum over the map's entries rather than one running number, because every
/// deletion path has to report what a folder held (`remove_session`,
/// `sweep_older_than`) and a running number cannot. See the note on
/// `StoreState::sessions`.
///
/// There is no key to filter by, and that is the point: the store is the unit
/// the budget is about, so every entry counts. One unknown folder makes the
/// whole answer `None` — a total that skipped the folder it could not read is a
/// number below the truth, and a budget check against a number below the truth
/// admits bytes the limit exists to refuse.
///
/// Saturating, like the wire's own sum of references: this total is a guard, and
/// a wrap would land it back under the limit it is meant to hold.
fn store_total(state: &StoreState) -> Option<u64> {
    // The root could not be listed, so the map is missing sessions rather than
    // reporting them: no total is knowable in this state.
    if state.root_unreadable {
        return None;
    }
    let mut total: u64 = 0;
    for bytes in state.sessions.values() {
        match bytes {
            SessionBytes::Known(bytes) => total = total.saturating_add(*bytes),
            SessionBytes::Unknown => return None,
        }
    }
    Some(total)
}

/// The bytes the cache attributes to one session: a number, a known zero for a
/// session with no folder, or `None` when the store cannot say.
///
/// `None` stands for two different silences that a caller acts on alike: the
/// folder is there and could not be read ([`SessionBytes::Unknown`]), or the
/// walk stopped short, so an absent entry is not evidence of absence. `Some(0)`
/// is a session with no folder in a picture that is complete, and a folder that
/// is there and holds nothing — the map does not tell those apart, and nothing
/// that reads it needs it to.
fn held_bytes(state: &StoreState, session_id: &str) -> Option<u64> {
    match state.sessions.get(session_id) {
        Some(SessionBytes::Known(bytes)) => Some(*bytes),
        Some(SessionBytes::Unknown) => None,
        // Not in the map. That is a known zero only when the map is the whole
        // picture: `seeded` is set by a walk that read everything, and left
        // false by one that did not.
        None if state.seeded => Some(0),
        None => None,
    }
}

/// The bytes of the files directly inside one session folder, or `None` if it
/// could not be listed.
///
/// Directly inside is all of it: this store writes files into
/// `<root>/<session id>/` and makes no subdirectory there, and a directory is
/// not bytes this store is holding. Whatever left a file there is counted, not
/// only what this process wrote — that is what lets the walk correct the cache
/// rather than confirm it, and a file an interrupted replace left behind is
/// bytes on the disk either way.
///
/// `None` and not zero. Zero is a claim about a folder and a failed listing is
/// the absence of one: the bytes that could not be read are exactly the bytes
/// the store would be handed as free budget, and nothing here bounds how many
/// there are. A folder that is *gone* is a different fact — there is nothing in
/// it, and `Some(0)` says so — so a folder deleted between the listing of the
/// root and this call is not reported as unknown, and neither is a file that
/// vanished between the listing and its own size.
fn folder_bytes(dir: &Path) -> Option<u64> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        // Gone is a known zero; there and unlistable is not a number at all.
        return if dir.exists() { None } else { Some(0) };
    };
    let mut total: u64 = 0;
    for entry in entries {
        // An entry that cannot be read makes the total unknown rather than
        // partial: a partial sum is missing bytes that are on the disk.
        let Ok(entry) = entry else {
            return None;
        };
        let Ok(metadata) = entry.metadata() else {
            // The same split as the folder above: gone held nothing to count,
            // there and unreadable is not a number.
            if entry.path().exists() {
                return None;
            }
            continue;
        };
        if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Some(total)
}

/// The newest modification time inside one session folder, the folder itself
/// included.
///
/// The folder's own timestamp is not enough: on a POSIX filesystem rewriting an
/// existing file does not touch its directory, so a session that only re-sent
/// files it had already stored would look idle.
///
/// The shape [`folder_bytes`] has, and for the same reason: an entry that cannot
/// be read makes the answer *unknown* rather than partial. This is the one that
/// decides a deletion, and a partial answer here deletes files — a folder whose
/// only unreadable entry was its newest write reported the newest write it could
/// read (`flatten` dropped the entry, `if let Ok` dropped its metadata), looked
/// idle, and was swept, contents and all. An unknown age is not a candidate: the
/// folder is kept and a later sweep decides.
///
/// Which read can fail is platform-dependent, and that is why there are two.
/// `DirEntry::metadata` is not a system call on Windows — the listing already
/// carries the attributes — so an entry is "readable" there even when its name
/// points at something that is gone, and it does not follow a link on POSIX
/// either. `fs::metadata` follows the name, so it is the read that fails on a
/// dangling entry, on both platforms. So the name has to resolve before its own
/// timestamp is used, and a folder holding one that does not is a folder this
/// store will not date.
///
/// # What this costs, written down
///
/// [`std::fs::metadata`] follows a link, so an entry whose name points at
/// something that is gone is one this function can never read *while it is
/// there*. A folder holding one is therefore never older than the limit as far
/// as the sweep is concerned: not "swept late", but **never expired at all**.
/// It stays until something other than the sweep removes it — a close
/// ([`AttachmentStore::remove_session`]), or the user — and nothing in this
/// store will reclaim it.
///
/// That is the trade taken on purpose, and the two errors are not symmetrical:
/// a folder that outlives its retention is visible (it is a folder that does not
/// go away) and recoverable (delete it), while a folder deleted on an age read
/// from the entries that *could* be read takes an attachment with it and cannot
/// be undone. A reader who finds a folder that never expires should read the
/// sentence above rather than look for the bug. [`folder_bytes`] keeps the same
/// trade on the budget side, where the same choice costs bytes instead of a
/// deletion.
fn newest_write(dir: &Path) -> Option<SystemTime> {
    let mut newest = std::fs::metadata(dir).ok()?.modified().ok()?;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries {
        // No `flatten` and no `if let Ok`: both spellings are this bug. An
        // entry the listing could not produce, an entry whose name does not
        // resolve, and an entry whose own metadata cannot be read are all
        // timestamps this function does not have, so it has no answer to give.
        let Ok(entry) = entry else {
            return None;
        };
        if std::fs::metadata(entry.path()).is_err() {
            return None;
        }
        let Ok(metadata) = entry.metadata() else {
            return None;
        };
        let Ok(modified) = metadata.modified() else {
            return None;
        };
        newest = newest.max(modified);
    }
    Some(newest)
}

/// Whether `dir`'s newest write is older than `max_age` as of `now`.
///
/// `None` is "cannot tell", and it is not a candidate. The folder could not be
/// read, or its newest write does not sit behind `now` — a timestamp in the
/// future is not evidence of age. Callers keep such a folder and let a later
/// sweep decide rather than deleting on a guess; an unreadable folder is not an
/// empty one.
fn is_older_than(dir: &Path, now: SystemTime, max_age: Duration) -> Option<bool> {
    let newest = newest_write(dir)?;
    let age = now.duration_since(newest).ok()?;
    Some(age > max_age)
}

#[cfg(test)]
#[path = "attachment_store_tests.rs"]
mod tests;
