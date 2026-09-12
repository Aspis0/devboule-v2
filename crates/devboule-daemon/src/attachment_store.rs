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
    ErrorCode, PromptAttachment, WireError, MAX_ATTACHMENT_OWNER_BYTES,
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
const STORED_EXTENSIONS: [&str; 3] = ["png", "jpg", "svg"];

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
    /// Serializes writes across sessions. One process owns the runtime dir
    /// (single-instance lock), so this is enough to keep two client threads
    /// materializing the same image from racing over the same temp file. It is
    /// also the lock the store's budget cache is taken with ([`StoreState`]), so
    /// a write and the total it moves are taken and left together.
    write_lock: Arc<Mutex<StoreState>>,
}

impl AttachmentStore {
    pub(crate) fn new(runtime_dir: &Path) -> Self {
        Self {
            root: runtime_dir.join(ATTACHMENTS_DIR),
            write_lock: Arc::new(Mutex::new(StoreState::default())),
        }
    }

    /// The folder holding one session's attachments.
    ///
    /// `None` for `.` and `..`. Both pass `validate_session_id` — its alphabet
    /// is `[A-Za-z0-9._-]` — and both are path traversal when joined to a root.
    /// A real id is composed as `s.<client token>.<n>`, and nothing here reads
    /// either segment: refusing these two loses nothing and removes the only way
    /// a session id could name a directory outside the store.
    pub(crate) fn session(&self, session_id: &str) -> Option<SessionAttachments> {
        if session_id == "." || session_id == ".." {
            return None;
        }
        Some(SessionAttachments {
            session_id: session_id.to_string(),
            dir: self.root.join(session_id),
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
        // `.` and `..` name no folder, so this call drops nothing and the caller
        // releases nothing: the same answer as an id whose folder is not there.
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
        self.seed_locked(&mut state);
        let held = held_bytes(&state, &session.session_id);
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
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            // No store yet is the normal state of a fresh install.
            return Vec::new();
        };
        let mut reclaimed: Vec<(String, Option<u64>)> = Vec::new();
        for entry in entries.flatten() {
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
        let key = dir.file_name().and_then(|name| name.to_str()).map(str::to_string);
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
    /// budget question. The exists check is therefore the first thing the guard
    /// does, and neither the limit nor an unknown total can refuse a file the
    /// session already has. The one refusal that comes before it is the id's: an
    /// id that names no folder is refused whether or not those bytes are already
    /// stored.
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
        self.seed_locked(&mut state);

        if path.exists() {
            // The bytes are already here: no file, no bytes, no budget
            // question. A store whose total cannot be computed still hands back
            // a file the session already holds.
            return Ok(Deposited {
                digest,
                stored_bytes: stored_size(&path)?,
                path,
            });
        }
        // A total that could not be computed is not a total of zero, and a
        // budget check against a number below the truth admits exactly the bytes
        // the limit exists to refuse.
        let Some(held) = store_total(&state) else {
            return Err(budget_unknown());
        };
        let after = held.saturating_add(stored.len() as u64);
        if after > MAX_ATTACHMENT_OWNER_BYTES as u64 {
            return Err(over_budget(after));
        }
        write_locked(&mut state, &session.session_id, &path, &stored)?;
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
    /// does; a caller showing a number has to say it does not know.
    ///
    /// It takes the store's write lock, and `std::sync::Mutex` is not
    /// reentrant. Calling it from inside a guarded section is a hang, which is
    /// why the deposit path does not call it: `deposit` asks `store_total` on
    /// the guard it already holds, because its check and the write it gates have
    /// to be one critical section. The two are the same sum.
    pub(crate) fn store_bytes(&self) -> Option<u64> {
        let mut state = self
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.seed_locked(&mut state);
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
    /// `None` is not zero, and it is the same two silences the total has: the
    /// session is one the walk could not read, or the root itself could not be
    /// listed. An id with no folder behind it is `Some(0)` — it holds nothing —
    /// and an id the store will not turn into a folder at all (`.` or `..`) is
    /// `None`, because there is no session to report on rather than an empty
    /// one.
    ///
    /// Same lock and the same warning as [`AttachmentStore::store_bytes`]: a
    /// `std::sync::Mutex` is not reentrant, and this takes the guard.
    pub(crate) fn session_bytes(&self, session_id: &str) -> Option<u64> {
        let Some(session) = self.session(session_id) else {
            return None;
        };
        let mut state = self
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.seed_locked(&mut state);
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
    fn seed_locked(&self, state: &mut StoreState) {
        if state.seeded {
            return;
        }
        state.sessions.clear();
        state.root_unreadable = false;
        // Set below, and only by a walk that read everything it looked at.
        let mut complete = true;
        match std::fs::read_dir(&self.root) {
            Ok(entries) => {
                for entry in entries {
                    // An entry that cannot be read makes the walk partial, and a
                    // partial walk is a picture of nothing: the folder it hides
                    // is a folder whose bytes would be missing from the total.
                    let Ok(entry) = entry else {
                        complete = false;
                        continue;
                    };
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
            Err(_) if !self.root.exists() => {}
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
/// `SessionDeposit` arm in `session.rs`, the piece still to land — the same
/// reason the block that returns this carries a dead-code marker.
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
        // The write charges the store's total as well. A file stored here is in
        // the folder the walk counts, and a cache that skipped this path would
        // under-count by every inline attachment this daemon ever stored through
        // it — which is the inline path's own unmetered bytes.
        write_locked(&mut guard, &self.session_id, &path, &stored)?;
        Ok(path)
    }
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
/// A write into a folder the walk could not read leaves that session
/// [`SessionBytes::Unknown`]. The bytes this process wrote are known and the
/// rest of the folder is not, so a total counting only the former would be the
/// same under-count the unknown state exists to remove.
fn write_locked(
    state: &mut StoreState,
    session_id: &str,
    path: &Path,
    stored: &[u8],
) -> Result<(), WireError> {
    if path.exists() {
        return Ok(());
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
/// which id it asked with. Only `.` and `..` reach this today — both pass
/// `validate_session_id`, and neither names a session — so the sentence is the
/// daemon's usual one for an id no session answers to.
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
        if path.is_file() {
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
        if names_the_digest && names_a_stored_file && path.is_file() {
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
fn newest_write(dir: &Path) -> Option<SystemTime> {
    let mut newest = std::fs::metadata(dir).ok()?.modified().ok()?;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        if let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) {
            newest = newest.max(modified);
        }
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
mod tests {
    use super::*;
    use crate::raster_metadata::{clean_png, png_with_text_chunk, vector_input, vector_output};
    use devboule_protocol::ATTACHMENT_MIME_TYPES;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let dir = std::env::temp_dir().join(format!(
                "devboule-attachments-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn attachment(name: &str, mime_type: &str, data: &str) -> PromptAttachment {
        PromptAttachment {
            name: name.to_string(),
            mime_type: mime_type.to_string(),
            data: data.to_string(),
        }
    }

    fn encoded(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn the_file_name_is_the_digest_of_the_bytes() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        // A container the walk accepts and changes nothing in, so the digest is
        // over exactly these bytes.
        let bytes = clean_png(0x01);

        let path = session
            .materialize(&attachment("photo.png", "image/png", &encoded(&bytes)))
            .expect("materialized");

        assert_eq!(
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string),
            Some(format!("{}.png", sha256_hex(&bytes)))
        );
        assert_eq!(std::fs::read(&path).expect("read"), bytes);
        assert!(path.starts_with(&temp.0));
    }

    #[test]
    fn the_users_name_never_reaches_the_path() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let bytes = clean_png(0x02);

        let path = session
            .materialize(&attachment(
                r"..\..\evil.png",
                "image/png",
                &encoded(&bytes),
            ))
            .expect("materialized");

        assert_eq!(path.parent(), Some(session.dir.as_path()));
        assert!(!path.to_string_lossy().contains("evil"));
        let siblings: Vec<_> = std::fs::read_dir(&temp.0)
            .expect("root list")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(siblings, vec!["attachments".to_string()]);
    }

    #[test]
    fn identical_bytes_are_stored_once() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let image = clean_png(0x03);
        let first = attachment("one.png", "image/png", &encoded(&image));
        let second = attachment("two.png", "image/png", &encoded(&image));

        let first_path = session.materialize(&first).expect("first");
        let second_path = session.materialize(&second).expect("second");

        assert_eq!(first_path, second_path);
        let files: Vec<_> = std::fs::read_dir(first_path.parent().expect("parent"))
            .expect("session list")
            .flatten()
            .collect();
        assert_eq!(files.len(), 1, "the same image left a second file behind");
    }

    #[test]
    fn different_bytes_get_different_files() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");

        let first = session
            .materialize(&attachment(
                "a.png",
                "image/png",
                &encoded(&clean_png(0x04)),
            ))
            .expect("first");
        let second = session
            .materialize(&attachment(
                "b.png",
                "image/png",
                &encoded(&clean_png(0x05)),
            ))
            .expect("second");

        assert_ne!(first, second);
    }

    #[test]
    fn a_stripped_file_is_named_after_the_bytes_that_were_written() {
        // The consequence this wiring carries, stated in the doc comment on
        // `materialize`: the name is the digest of what is on disk, not of what
        // arrived, so a client that predicts the path from its own digest is
        // wrong whenever a rule fired.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let sent = png_with_text_chunk();
        let kept = clean_png(0x01);
        assert_ne!(
            sha256_hex(&sent),
            sha256_hex(&kept),
            "the fixture must actually carry something that leaves"
        );

        let path = session
            .materialize(&attachment("photo.png", "image/png", &encoded(&sent)))
            .expect("materialized");

        assert_eq!(
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string),
            Some(format!("{}.png", sha256_hex(&kept)))
        );
        assert_eq!(std::fs::read(&path).expect("read"), kept);
        assert!(
            !path.to_string_lossy().contains(sha256_hex(&sent).as_str()),
            "the digest of the bytes the client sent is not the path"
        );
    }

    #[test]
    fn an_image_the_walk_cannot_follow_is_refused_rather_than_written() {
        // Real PNG bytes, cut short inside the last chunk. The sniff agrees with
        // the label here, so this reaches the walk and fails there rather than
        // at the disagreement check below — which is the path this test is
        // about, and the one the fixture used to reach before the sniff existed.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let mut truncated = clean_png(0x13);
        truncated.truncate(truncated.len() - 6);

        let item = attachment("photo.png", "image/png", &encoded(&truncated));
        let error = session.materialize(&item).expect_err("refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("could not be read"),
            "{}",
            error.message
        );
        assert!(!session.dir.exists(), "nothing may be created on a refusal");
    }

    /// The vector the JPEG-side tests use: real bytes carrying EXIF, taken from
    /// the shared file rather than hand-rolled, so a test that asserts about
    /// stripping is asserting about the same bytes the rule is pinned to.
    const EXIF_JPEG_VECTOR: &str =
        "a jpeg whose APP1 holds EXIF, between a kept JFIF APP0 and a kept ICC APP2";

    #[test]
    fn a_jpeg_carrying_exif_declared_as_svg_is_refused() {
        // The regression test for the bypass. The strip used to be selected by
        // `mime_type` — a field the sender controls — so this exact file, a JPEG
        // whose APP1 holds GPS coordinates, took the `image/svg+xml`
        // write-through path and was stored untouched. Nothing may be written:
        // the bytes were never sanitised, and a path to them is a promise the
        // daemon cannot keep.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let jpeg = vector_input(EXIF_JPEG_VECTOR);

        let item = attachment("photo.jpg", "image/svg+xml", &encoded(&jpeg));
        let error = session.materialize(&item).expect_err("refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("do not match the type"),
            "{}",
            error.message
        );
        assert!(!session.dir.exists(), "nothing may be created on a refusal");
    }

    #[test]
    fn a_png_declared_as_a_jpeg_is_refused() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");

        let item = attachment("photo.png", "image/jpeg", &encoded(&clean_png(0x14)));
        let error = session.materialize(&item).expect_err("refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(!session.dir.exists(), "nothing may be created on a refusal");
    }

    #[test]
    fn a_declared_raster_whose_bytes_are_not_a_container_is_refused() {
        // The label says PNG and the bytes say nothing recognisable, so there is
        // no walk to run and no evidence to check the label against. A file that
        // cannot be walked is refused rather than stored, which is the rule the
        // whole pass rests on.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");

        let item = attachment("photo.png", "image/png", &encoded(b"not a png at all"));
        let error = session.materialize(&item).expect_err("refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(!session.dir.exists(), "nothing may be created on a refusal");
    }

    #[test]
    fn a_correctly_declared_jpeg_still_strips() {
        // The counterweight to the refusals: when the label and the bytes agree,
        // the walk runs and what lands on disk is the vector's authored output,
        // named after it. Without this, a "fix" that refused everything would
        // look like it passed the tests above.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let sent = vector_input(EXIF_JPEG_VECTOR);
        let kept = vector_output(EXIF_JPEG_VECTOR);
        assert_ne!(
            sha256_hex(&sent),
            sha256_hex(&kept),
            "the vector must actually lose something"
        );

        let item = attachment("photo.jpg", "image/jpeg", &encoded(&sent));
        let path = session.materialize(&item).expect("materialized");

        assert_eq!(
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string),
            Some(format!("{}.jpg", sha256_hex(&kept)))
        );
        assert_eq!(std::fs::read(&path).expect("read"), kept);
    }

    #[test]
    fn an_svg_is_written_as_it_arrived() {
        // The frontend sanitises SVG source and this side does not, so an SVG
        // that arrives from another device is stored unsanitised. Pinning that
        // keeps the gap visible: if this ever fails because the bytes changed,
        // a sanitiser was added and the doc comment on `materialize` is wrong.
        //
        // These bytes are neither container, which is what makes this the one
        // remaining write-through path: nothing here is examined, so the label
        // is the only thing that decides what is written — including its
        // extension.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let sent = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";

        let path = session
            .materialize(&attachment("drawing.svg", "image/svg+xml", &encoded(sent)))
            .expect("materialized");

        assert_eq!(std::fs::read(&path).expect("read"), sent.to_vec());
    }

    #[test]
    fn every_supported_type_has_an_extension() {
        // Pins the store's table to the wire's list: a type that validates but
        // has no extension would be refused after the validator accepted it.
        for mime_type in ATTACHMENT_MIME_TYPES {
            assert!(
                extension_for(mime_type).is_some(),
                "{mime_type} is accepted on the wire but has no extension"
            );
        }
    }

    #[test]
    fn a_session_id_that_is_a_parent_directory_is_refused() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        assert!(store.session("..").is_none());
        assert!(store.session(".").is_none());
        assert_eq!(
            store.remove_session(".."),
            Some(0),
            "an id with no folder dropped nothing"
        );
        assert!(temp.0.exists(), "the store must not walk out of its root");
    }

    #[test]
    fn invalid_base64_is_refused_rather_than_written() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let error = session
            .materialize(&attachment("a.png", "image/png", "not base64!"))
            .expect_err("refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, invalid_base64_message());
        assert!(!session.dir.exists(), "nothing may be created on a refusal");
    }

    #[test]
    fn an_unsupported_type_is_refused_rather_than_named_png() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let error = session
            .materialize(&attachment("a.gif", "image/gif", &encoded(b"gif")))
            .expect_err("refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("image/gif"), "{}", error.message);
    }

    #[test]
    fn retention_deletes_a_folder_older_than_the_limit() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let path = session
            .materialize(&attachment(
                "a.png",
                "image/png",
                &encoded(&clean_png(0x06)),
            ))
            .expect("materialized");

        let later = SystemTime::now() + ATTACHMENT_RETENTION + Duration::from_secs(60);
        let reclaimed = store.sweep_older_than(later, ATTACHMENT_RETENTION);
        assert_eq!(reclaimed.len(), 1, "one folder was past the limit");
        assert_eq!(reclaimed[0].0, "s.a.1", "the report names the session");
        assert_eq!(
            reclaimed[0].1,
            Some(clean_png(0x06).len() as u64),
            "and what its removal took out of the total"
        );
        assert!(!path.exists(), "the file must go with its folder");
        assert!(!session.dir.exists());
    }

    #[test]
    fn retention_keeps_a_folder_inside_the_limit() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let path = session
            .materialize(&attachment(
                "a.png",
                "image/png",
                &encoded(&clean_png(0x07)),
            ))
            .expect("materialized");

        let soon = SystemTime::now() + Duration::from_secs(60);
        assert!(store
            .sweep_older_than(soon, ATTACHMENT_RETENTION)
            .is_empty());
        assert!(path.exists());
    }

    #[test]
    fn retention_keeps_a_folder_whose_timestamp_is_in_the_future() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let path = session
            .materialize(&attachment(
                "a.png",
                "image/png",
                &encoded(&clean_png(0x08)),
            ))
            .expect("materialized");
        let file = std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("open");
        file.set_modified(SystemTime::now() + Duration::from_secs(86_400))
            .expect("set mtime");

        assert!(store
            .sweep_older_than(SystemTime::now(), ATTACHMENT_RETENTION)
            .is_empty());
        assert!(path.exists());
    }

    #[test]
    fn sweeping_a_store_that_does_not_exist_yet_is_not_an_error() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0.join("absent"));
        assert!(store
            .sweep_older_than(SystemTime::now(), ATTACHMENT_RETENTION)
            .is_empty());
    }

    #[test]
    fn closing_a_session_removes_only_that_session() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let kept = store.session("s.a.2").expect("session");
        let kept_path = kept
            .materialize(&attachment(
                "b.png",
                "image/png",
                &encoded(&clean_png(0x09)),
            ))
            .expect("materialized");
        store
            .session("s.a.1")
            .expect("session")
            .materialize(&attachment(
                "a.png",
                "image/png",
                &encoded(&clean_png(0x0a)),
            ))
            .expect("materialized");

        store.remove_session("s.a.1");

        assert!(!store.session("s.a.1").expect("session").dir.exists());
        assert!(kept_path.exists());
    }

    #[test]
    fn an_under_lock_recheck_keeps_a_folder_that_became_fresh() {
        // The sweep decides on age without the lock and deletes under it, and
        // the decision is made again there. This pins the second decision: a
        // folder that was old enough when the unlocked filter ran, and had a
        // write land before the lock, is kept — deleting it would lose an
        // attachment the user made moments ago.
        //
        // The interleaving with a real writer is not reproduced, and cannot be
        // pinned here: the only synchronization point between the unlocked
        // filter and the removal is the lock wait itself, which std offers no
        // way to observe. A second thread told to "materialize while the sweep
        // is parked" would have to be held back by a sleep, which is a guess
        // rather than an assertion. What is pinned is the rule the removal
        // runs under the lock.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let path = session
            .materialize(&attachment(
                "a.png",
                "image/png",
                &encoded(&clean_png(0x0d)),
            ))
            .expect("materialized");

        // `now` is a parameter of the sweep, so the folder can be made old by
        // moving `now` forward instead of moving file timestamps back.
        let now = SystemTime::now();
        let sweep_now = now + ATTACHMENT_RETENTION + Duration::from_secs(120);

        // What the unlocked filter sees: a candidate.
        assert_eq!(
            is_older_than(&session.dir, sweep_now, ATTACHMENT_RETENTION),
            Some(true)
        );

        // A write lands in the folder while the sweep is on its way to the
        // lock, so its newest write moves in front of the limit.
        let file = std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("open");
        file.set_modified(sweep_now - Duration::from_secs(30))
            .expect("set mtime");
        assert_eq!(
            is_older_than(&session.dir, sweep_now, ATTACHMENT_RETENTION),
            Some(false)
        );

        assert!(
            store
                .remove_if_still_older_than(&session.dir, sweep_now, ATTACHMENT_RETENTION)
                .is_none(),
            "a folder that is no longer old must not be removed under the lock"
        );
        assert!(path.exists(), "the write's folder must survive");
    }

    #[test]
    fn an_under_lock_recheck_keeps_a_path_it_cannot_read() {
        // An unreadable folder is not an empty one. When `newest_write` cannot
        // read the path — here a file where a folder is expected, the portable
        // stand-in for any path that cannot be listed — the answer is "cannot
        // tell", and the removal refuses instead of deleting on a guess.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let stray = temp.0.join("not-a-folder");
        std::fs::write(&stray, b"x").expect("write");

        let now = SystemTime::now() + ATTACHMENT_RETENTION + Duration::from_secs(120);
        assert_eq!(is_older_than(&stray, now, ATTACHMENT_RETENTION), None);
        assert!(
            store
                .remove_if_still_older_than(&stray, now, ATTACHMENT_RETENTION)
                .is_none(),
            "a path whose age cannot be read must not be removed"
        );
        assert!(stray.exists(), "nothing may be deleted on a guess");
    }

    /// How long the tests below give a deleter that should be blocked before
    /// they call it blocked. The value is generous on purpose: under the fix
    /// the deleter cannot proceed while this thread holds the lock, so no
    /// timeout can make the assertion fail, and a build without the lock
    /// answers as soon as its thread is scheduled. See the comment on
    /// `closing_a_session_waits_for_an_in_flight_write` for why the wait is a
    /// timeout and not a sleep.
    const BLOCKED_TIMEOUT: Duration = Duration::from_millis(500);

    #[test]
    fn closing_a_session_waits_for_an_in_flight_write() {
        // The refusal to race is asserted as serialization: hold the write
        // lock, ask another thread to close the session, and require that the
        // close does not finish while the lock is held. The wait is a channel
        // timeout, which is the shape of the statement and not a sleep — a
        // fixed build cannot satisfy `recv_timeout` here because the deleter
        // is parked on a mutex, while a build missing the lock sends `done` as
        // soon as the thread runs. The `started` message makes the assertion
        // about the close itself rather than about thread scheduling.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let path = session
            .materialize(&attachment(
                "a.png",
                "image/png",
                &encoded(&clean_png(0x0b)),
            ))
            .expect("materialized");

        let guard = store
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let closer = {
            let store = store.clone();
            std::thread::spawn(move || {
                started_tx.send(()).expect("signal start");
                store.remove_session("s.a.1");
                done_tx.send(()).expect("signal done");
            })
        };

        started_rx.recv().expect("the closer started");
        assert!(
            done_rx.recv_timeout(BLOCKED_TIMEOUT).is_err(),
            "the close finished while a writer held the lock"
        );
        assert!(path.exists(), "the folder was removed under the lock");

        drop(guard);
        done_rx
            .recv()
            .expect("the close finished once the lock was free");
        closer.join().expect("thread");
        assert!(!path.exists(), "the close must still remove the folder");
    }

    #[test]
    fn a_retention_sweep_waits_for_an_in_flight_write() {
        // The sweep takes the lock per removal. The observable half of that is
        // the same serialization the close shows: while this thread holds the
        // lock, a sweep that has decided to delete the folder cannot finish.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let path = session
            .materialize(&attachment(
                "a.png",
                "image/png",
                &encoded(&clean_png(0x0c)),
            ))
            .expect("materialized");

        let guard = store
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let sweeper = {
            let store = store.clone();
            std::thread::spawn(move || {
                started_tx.send(()).expect("signal start");
                let later = SystemTime::now() + ATTACHMENT_RETENTION + Duration::from_secs(60);
                let removed = store.sweep_older_than(later, ATTACHMENT_RETENTION).len();
                done_tx.send(removed).expect("signal done");
            })
        };

        started_rx.recv().expect("the sweeper started");
        assert!(
            done_rx.recv_timeout(BLOCKED_TIMEOUT).is_err(),
            "the sweep finished while a writer held the lock"
        );
        assert!(path.exists(), "the folder was removed under the lock");

        drop(guard);
        assert_eq!(
            done_rx
                .recv()
                .expect("the sweep finished once the lock was free"),
            1,
            "the sweep still reports the folder it removed"
        );
        sweeper.join().expect("thread");
        assert!(!path.exists(), "the sweep must still remove the folder");
    }

    #[test]
    fn the_fingerprint_digest_distinguishes_two_images_with_one_name() {
        let first = attachment("same.png", "image/png", &encoded(b"first image"));
        let second = attachment("same.png", "image/png", &encoded(b"second image"));
        assert_ne!(attachment_digest(&first), attachment_digest(&second));
        assert_eq!(attachment_digest(&first), attachment_digest(&first));
        assert_eq!(attachment_digest(&first).len(), 64);
    }

    #[test]
    fn depositing_the_same_bytes_twice_in_one_session_counts_once() {
        // The within-session half of content addressing. The second deposit
        // finds the digest's file already there, so it creates no file and must
        // move no total: an increment charged beside the budget check rather
        // than after the write passes every other test in this file and fails
        // this one.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let bytes = clean_png(0x21);
        let item = attachment("photo.png", "image/png", &encoded(&bytes));

        let first = store.deposit("s.a.1", &item).expect("first deposit");
        let second = store.deposit("s.a.1", &item).expect("second deposit");

        assert_eq!(first.digest, second.digest);
        assert_eq!(first.path, second.path);
        assert_eq!(first.stored_bytes, bytes.len() as u64);
        assert_eq!(
            store.store_bytes(),
            Some(bytes.len() as u64),
            "one file, one increment"
        );
        assert_eq!(
            std::fs::read_dir(first.path.parent().expect("session folder"))
                .expect("read the session folder")
                .flatten()
                .count(),
            1,
            "the second deposit left a second file behind"
        );
    }

    #[test]
    fn the_same_bytes_in_two_sessions_count_twice() {
        // The case an auditor got wrong. Content addressing is scoped to one
        // session folder, so the second session's digest names a second file and
        // a second contribution to the store's total. A cache keyed by digest
        // alone — or a lookup that searched outside the session's folder — would
        // report one copy here, and the store would be allowed twice the bytes
        // the limit is for.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let bytes = clean_png(0x22);
        let item = attachment("photo.png", "image/png", &encoded(&bytes));

        let first = store.deposit("s.a.1", &item).expect("first session");
        let second = store.deposit("s.a.2", &item).expect("second session");

        assert_eq!(first.digest, second.digest, "one image, one digest");
        assert_ne!(first.path, second.path, "two sessions, two files");
        assert!(second.path.exists());
        assert_eq!(store.store_bytes(), Some(2 * bytes.len() as u64));
        // Per session as well as in total, because the caller that reserves
        // bytes against a device attributes them one session at a time.
        assert_eq!(store.session_bytes("s.a.1"), Some(bytes.len() as u64));
        assert_eq!(store.session_bytes("s.a.2"), Some(bytes.len() as u64));
    }

    #[test]
    fn closing_a_session_drops_only_its_own_bytes_from_the_store_total() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let kept = store
            .deposit(
                "s.a.2",
                &attachment("kept.png", "image/png", &encoded(&clean_png(0x23))),
            )
            .expect("kept");
        let gone = store
            .deposit(
                "s.a.1",
                &attachment("gone.png", "image/png", &encoded(&clean_png(0x24))),
            )
            .expect("gone");
        assert_eq!(
            store.store_bytes(),
            Some(kept.stored_bytes + gone.stored_bytes)
        );

        assert_eq!(
            store.remove_session("s.a.1"),
            Some(gone.stored_bytes),
            "the close reports what it dropped"
        );

        assert_eq!(
            store.store_bytes(),
            Some(kept.stored_bytes),
            "the close must drop exactly one session's bytes"
        );
        assert!(kept.path.exists(), "the sibling session's file was removed");
        assert!(!gone.path.exists());
    }

    #[test]
    fn an_over_budget_deposit_writes_nothing() {
        // The budget is read from the tree, so the cheapest fixture that fills
        // it is a folder this process never wrote: the state D4 calls "a folder
        // present at startup", where the walk is the only thing that knows the
        // bytes are there. That assertion doubles as the proof that the walk
        // counts what it finds rather than only what a deposit recorded.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let full = store.session("s.a.1").expect("session");
        std::fs::create_dir_all(&full.dir).expect("session folder");
        std::fs::write(
            full.dir.join("photo.png"),
            vec![0u8; MAX_ATTACHMENT_OWNER_BYTES],
        )
        .expect("fill the store's budget");

        assert_eq!(
            store.store_bytes(),
            Some(MAX_ATTACHMENT_OWNER_BYTES as u64),
            "the walk must count a folder this process did not write"
        );

        let bytes = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        let item = attachment("drawing.svg", "image/svg+xml", &encoded(bytes));
        let error = store.deposit("s.a.2", &item).expect_err("refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error
                .message
                .contains(&MAX_ATTACHMENT_OWNER_BYTES.to_string()),
            "the refusal must name the limit it refused against: {}",
            error.message
        );
        assert!(
            !store.session("s.a.2").expect("session").dir.exists(),
            "nothing may be created on a refusal"
        );

        // The refusal is the budget and not the shape of the request: once the
        // folder is gone the same bytes are accepted, which is the close
        // dropping exactly one session's contribution.
        assert_eq!(
            store.remove_session("s.a.1"),
            Some(MAX_ATTACHMENT_OWNER_BYTES as u64),
            "the close reports the bytes the filled folder held"
        );
        assert_eq!(store.store_bytes(), Some(0));
        let accepted = store.deposit("s.a.2", &item).expect("accepted");
        assert!(accepted.path.exists());
        assert_eq!(accepted.stored_bytes, bytes.len() as u64);
        assert_eq!(store.store_bytes(), Some(bytes.len() as u64));
    }

    #[test]
    fn a_digest_that_is_not_a_digest_is_refused() {
        // The store is the second door. The wire checks a reference's digest
        // before anything here is reached, and this must not depend on that
        // check having run: every string below would name something once it were
        // joined to a session folder, and none of them may reach the filesystem
        // at all.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let refusals = [
            // A traversal, at the length the check above the join expects.
            "../".repeat(21) + "a",
            // Right length, wrong characters: not hex, and uppercase hex.
            "z".repeat(64),
            "A".repeat(64),
            // Right characters, wrong length: one short, one long.
            "a".repeat(63),
            "a".repeat(65),
            // A separator inside a string that is otherwise a digest.
            format!("/{}", "a".repeat(63)),
        ];

        for digest in &refusals {
            let error = store
                .resolve("s.a.1", digest, Some("png"))
                .expect_err("refused");
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{digest}");
            assert_eq!(
                error.message,
                invalid_attachment_digest_message(),
                "{digest}"
            );
        }

        // Nothing was looked up, so nothing was created: not the session folder,
        // and not anything a traversal would have climbed to from it.
        assert!(
            !temp.0.join(ATTACHMENTS_DIR).exists(),
            "a refusal may not create the path it refused"
        );

        // A well-formed digest under a session id the store will not turn into
        // a folder is refused as well, before a name is built from either.
        assert!(store.resolve("..", &"a".repeat(64), None).is_err());
    }

    #[test]
    fn a_digest_with_no_file_behind_it_is_refused() {
        // A refusal and not an empty success: a missing file is not a stored
        // attachment of zero bytes, and a caller handed `Ok` with a path would
        // find that out at the provider instead, one layer further from the
        // cause than the request that named it.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let absent = sha256_hex(b"never deposited");

        let error = store
            .resolve("s.a.1", &absent, Some("png"))
            .expect_err("refused");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_ne!(
            error.message,
            invalid_attachment_digest_message(),
            "the digest is well formed; the file is what is missing"
        );
    }

    #[test]
    fn a_resolved_file_reports_the_size_the_store_wrote() {
        // The size a resolve hands back is read from the file, so it is the
        // stored size and not the size that was sent: this fixture carries a text
        // chunk the strip removes, which is exactly the disagreement D2 turns
        // into a refusal when the client's own number is the one that travelled.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let sent = png_with_text_chunk();
        let kept = clean_png(0x01);
        let deposited = store
            .deposit(
                "s.a.1",
                &attachment("photo.png", "image/png", &encoded(&sent)),
            )
            .expect("deposited");

        assert_eq!(deposited.stored_bytes, kept.len() as u64);
        assert_ne!(
            deposited.stored_bytes,
            sent.len() as u64,
            "the fixture must lose bytes to the strip"
        );

        let (path, stored_bytes) = store
            .resolve("s.a.1", &deposited.digest, Some("png"))
            .expect("resolved");
        assert_eq!(path, deposited.path);
        assert_eq!(stored_bytes, kept.len() as u64);
        assert_eq!(
            stored_bytes,
            std::fs::metadata(&path).expect("metadata").len(),
            "the size must be the file's own"
        );

        // The hint is a shortcut and not the answer: with none, the folder's
        // listing finds the same file, which is the path a caller holding only a
        // session and a digest takes.
        let (listed, listed_bytes) = store
            .resolve("s.a.1", &deposited.digest, None)
            .expect("resolved without a hint");
        assert_eq!(listed, path);
        assert_eq!(listed_bytes, stored_bytes);

        // A hint for the wrong type falls back to the listing rather than
        // refusing, or answering about a file that is not there.
        let (wrong_hint, _) = store
            .resolve("s.a.1", &deposited.digest, Some("image/svg+xml"))
            .expect("resolved with the wrong hint");
        assert_eq!(wrong_hint, path);

        // A digest resolves only in the session it was deposited to, so the same
        // digest asked about from another session is a refusal.
        assert!(store
            .resolve("s.a.2", &deposited.digest, Some("png"))
            .is_err());
    }

    #[test]
    fn the_inline_path_writes_for_a_legacy_id_and_counts_the_bytes() {
        // The M2 form has no middle segment to read, and nothing reads one any
        // more. The inline path keeps writing for it, and its bytes are counted
        // like any other file's: the write goes through `write_locked`, which
        // charges the folder it landed in. Before that, an inline attachment was
        // bytes the store held that no number anywhere knew about.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("session-123-1").expect("session");
        let bytes = clean_png(0x35);
        let path = session
            .materialize(&attachment("photo.png", "image/png", &encoded(&bytes)))
            .expect("materialized");

        assert!(path.exists(), "the inline path must still store the file");
        assert_eq!(
            store.session_bytes("session-123-1"),
            Some(bytes.len() as u64),
            "the file the inline path wrote is counted"
        );
        assert_eq!(
            store.store_bytes(),
            Some(bytes.len() as u64),
            "and it is part of the store's total"
        );
    }

    #[test]
    fn a_folder_that_cannot_be_listed_has_no_size() {
        // A file where a folder is expected is this file's stand-in for a path
        // that cannot be listed (`an_under_lock_recheck_keeps_a_path_it_cannot_read`),
        // and it is a real failure rather than an injected one. The point is the
        // difference between "could not read" and "nothing there": both are a
        // failed listing, and only one of them is a zero.
        let temp = TempDir::new();
        let stray = temp.0.join("not-a-folder");
        std::fs::write(&stray, b"x").expect("write");

        assert_eq!(
            folder_bytes(&stray),
            None,
            "a listing that failed is not an empty folder"
        );
        assert_eq!(
            folder_bytes(&temp.0.join("absent")),
            Some(0),
            "a folder that is gone holds nothing, which is a number"
        );
    }

    #[test]
    fn a_root_that_cannot_be_listed_makes_every_total_unknown() {
        // The same failure as the folder above, one level up — and this one is
        // produced for real rather than injected: `attachments` exists and is not
        // a folder, so the walk cannot learn which sessions exist and no total is
        // knowable. Every budget question answers unknown, which is the honest
        // answer when a store cannot read its own root, and the deposit is
        // refused before it writes, so an unreadable root does not become the
        // hole instead.
        let temp = TempDir::new();
        let state_file = temp.0.join(ATTACHMENTS_DIR);
        std::fs::write(&state_file, b"not a folder").expect("write");

        let store = AttachmentStore::new(&temp.0);
        assert_eq!(
            store.store_bytes(),
            None,
            "a root that cannot be listed is not an empty store"
        );
        let error = store
            .deposit(
                "s.a.1",
                &attachment("photo.png", "image/png", &encoded(&clean_png(0x36))),
            )
            .expect_err("refused");
        assert_eq!(error.code, ErrorCode::Io);
        assert!(
            error.message.contains("could not be read"),
            "the refusal must name the real cause: {}",
            error.message
        );
        assert_eq!(
            std::fs::read(&state_file).expect("read"),
            b"not a folder".to_vec(),
            "nothing may be written where the store root belongs"
        );
    }

    /// Put one session into the state a folder the walk could not read leaves it
    /// in, without needing an unreadable folder to exist.
    ///
    /// [`SessionBytes::Unknown`] is reachable only from a `read_dir` that failed,
    /// and there is no portable way to make one fail in a test: on Windows it
    /// takes an ACL or a held exclusive handle on the directory, neither of which
    /// a test may assume it is allowed to create, and on a POSIX box a
    /// `chmod 000` on the folder does nothing when the suite runs as root. So the
    /// test writes the state the walk would have reached — through the store's own
    /// lock, after a first seed, so every later `seed_locked` is a no-op and the
    /// entry stands — and asserts the consequence, which is the part the store is
    /// responsible for. What this does *not* pin is `seed_locked` declining to set
    /// `seeded`: that needs the real failure, and the comment above the assertion
    /// in `an_unknown_folder_refuses_every_deposit` says so.
    fn make_unknown(store: &AttachmentStore, session_id: &str) {
        let mut state = store
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        store.seed_locked(&mut state);
        state
            .sessions
            .insert(session_id.to_string(), SessionBytes::Unknown);
    }

    /// Clear `seeded`, which is the state a walk that could not read a folder
    /// leaves behind (`seed_locked`): the next budget question walks again and
    /// rebuilds the map from the tree.
    ///
    /// Same caveat as `make_unknown` — the real state comes from a failed
    /// `read_dir`, which a test cannot produce portably — so the test sets the
    /// flag the walk would have left and asserts what the store does with it.
    fn make_unseeded(store: &AttachmentStore) {
        let mut state = store
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.seeded = false;
    }

    #[test]
    fn an_unknown_folder_refuses_every_deposit() {
        // One folder the walk could not read makes the total unknown, and an
        // unknown total is a refusal rather than a number rounded down: the bytes
        // in that folder are exactly the budget a zero would hand back. The
        // refusal is store-wide because the budget is — every deposit is checked
        // against the same number, so while that number is unknowable every
        // deposit is refused, including deposits into sessions whose own folders
        // read perfectly well. That is what one budget instead of one per
        // connection costs, and the rebuilt picture is what lifts it.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let balanced = store
            .deposit(
                "s.b.1",
                &attachment("b.png", "image/png", &encoded(&clean_png(0x31))),
            )
            .expect("an unrelated session's deposit");

        make_unknown(&store, "s.a.1");

        assert_eq!(
            store.store_bytes(),
            None,
            "an unreadable folder is not a zero"
        );
        for session_id in ["s.a.1", "s.b.2"] {
            let error = store
                .deposit(
                    session_id,
                    &attachment("a.png", "image/png", &encoded(&clean_png(0x32))),
                )
                .expect_err("refused");
            assert_eq!(error.code, ErrorCode::Io, "{session_id}");
            assert!(
                error.message.contains("could not be read"),
                "the refusal must name the real cause: {}",
                error.message
            );
            assert!(
                !store.session(session_id).expect("session").dir.exists(),
                "nothing may be created on a refusal: {session_id}"
            );
        }

        // And it lifts the way the design says it does: a picture the walk has
        // rebuilt has no unknown in it, and what was refused is accepted.
        make_unseeded(&store);
        assert_eq!(store.store_bytes(), Some(balanced.stored_bytes));
        let accepted = store
            .deposit(
                "s.a.1",
                &attachment("a.png", "image/png", &encoded(&clean_png(0x32))),
            )
            .expect("accepted once the picture is rebuilt");
        assert!(accepted.path.exists());
    }

    #[test]
    fn two_sessions_with_unrelated_id_shapes_count_toward_one_total() {
        // Why there is no key. One id carries a middle segment that looks like an
        // owner and is a connection token; another is the M2 form, which has no
        // middle segment at all. Both are folders in one store, and both count
        // against one budget: a total keyed on that segment would have put these
        // in different buckets, which is how one user's twenty megabytes became
        // two budgets for two clients.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let bytes = clean_png(0x41);
        let item = attachment("photo.png", "image/png", &encoded(&bytes));

        let token = store
            .deposit("s.a.1", &item)
            .expect("a session with a token");
        let other = store
            .deposit("s.b.1", &item)
            .expect("a session with a different token");
        let legacy = store
            .deposit("session-123-1", &item)
            .expect("a session with no token at all");

        assert_eq!(token.digest, other.digest, "one image, one digest");
        assert_eq!(legacy.digest, token.digest);
        assert_eq!(
            store.store_bytes(),
            Some(3 * bytes.len() as u64),
            "three folders, three copies, one budget"
        );
        assert_eq!(
            store.session_bytes("session-123-1"),
            Some(bytes.len() as u64)
        );
        assert!(legacy.path.exists());
    }

    #[test]
    fn session_bytes_answers_for_one_session_or_not_at_all() {
        // The reader a caller attributes bytes with when it keeps a counter per
        // device rather than one for the store.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let bytes = clean_png(0x42);
        let deposited = store
            .deposit(
                "s.a.1",
                &attachment("photo.png", "image/png", &encoded(&bytes)),
            )
            .expect("deposited");

        assert_eq!(store.session_bytes("s.a.1"), Some(deposited.stored_bytes));
        assert_eq!(
            store.session_bytes("s.a.2"),
            Some(0),
            "a session with no folder holds nothing, which is a number"
        );
        assert_eq!(
            store.session_bytes(".."),
            None,
            "an id the store will not turn into a folder is not an empty session"
        );

        make_unknown(&store, "s.a.2");
        assert_eq!(
            store.session_bytes("s.a.2"),
            None,
            "a folder that could not be read is unknown, not zero"
        );
        assert_eq!(
            store.session_bytes("s.a.1"),
            Some(deposited.stored_bytes),
            "and one unknown folder does not taint a session that was read"
        );
    }

    #[test]
    fn closing_a_session_reports_what_it_dropped() {
        // The number a caller subtracts from what it had reserved.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let first = store
            .deposit(
                "s.a.1",
                &attachment("a.png", "image/png", &encoded(&clean_png(0x43))),
            )
            .expect("first");
        let second = store
            .deposit(
                "s.a.2",
                &attachment("b.png", "image/png", &encoded(&clean_png(0x44))),
            )
            .expect("second");

        assert_eq!(
            store.remove_session("s.a.1"),
            Some(first.stored_bytes),
            "the bytes the folder held"
        );
        assert_eq!(store.store_bytes(), Some(second.stored_bytes));
        assert!(second.path.exists(), "the other session's folder stays");

        // A second close, a session that never existed, and an id with no folder
        // all dropped nothing, and they answer `Some(0)` rather than `None`:
        // `None` means "do not release, the store cannot say", so a caller that
        // read it here would hold a reservation it should have released.
        assert_eq!(store.remove_session("s.a.1"), Some(0));
        assert_eq!(store.remove_session("s.never.9"), Some(0));
        assert_eq!(store.remove_session(".."), Some(0));

        // Unknown is the one answer a caller must not act on.
        make_unknown(&store, "s.a.3");
        assert_eq!(
            store.remove_session("s.a.3"),
            None,
            "a folder the store could not count reports no number"
        );
        assert_eq!(store.store_bytes(), Some(second.stored_bytes));
    }

    #[test]
    fn a_sweep_reports_what_it_reclaimed() {
        // The sweep takes bytes out of the store with no caller asking, and it is
        // the path a per-device counter cannot see unless the store says what
        // went: a folder deleted here is a reservation elsewhere that nothing
        // released. So every removal travels with the session whose folder it was
        // and the bytes it reclaimed.
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let swept = store
            .deposit(
                "s.a.1",
                &attachment("a.png", "image/png", &encoded(&clean_png(0x45))),
            )
            .expect("swept");
        let kept = store
            .deposit(
                "s.a.2",
                &attachment("b.png", "image/png", &encoded(&clean_png(0x46))),
            )
            .expect("kept");

        // The sweep's clock is `later`, so the kept folder has to be newer than
        // the limit as measured against it: touching the file it holds puts it
        // there, the same trick successive retention tests use.
        let file = std::fs::File::options()
            .write(true)
            .open(&kept.path)
            .expect("open");
        file.set_modified(SystemTime::now() + ATTACHMENT_RETENTION + Duration::from_secs(30))
            .expect("set mtime");

        let later = SystemTime::now() + ATTACHMENT_RETENTION + Duration::from_secs(60);
        let reclaimed = store.sweep_older_than(later, ATTACHMENT_RETENTION);

        assert_eq!(reclaimed.len(), 1, "one folder was past the limit");
        assert_eq!(reclaimed[0].0, "s.a.1", "the report names the session");
        assert_eq!(
            reclaimed[0].1,
            Some(swept.stored_bytes),
            "and the bytes it reclaimed"
        );
        assert!(kept.path.exists(), "the fresh folder stays");
        assert_eq!(
            store.store_bytes(),
            Some(kept.stored_bytes),
            "what the sweep reclaimed has left the total"
        );
    }
}
