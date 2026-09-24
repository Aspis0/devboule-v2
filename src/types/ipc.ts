export type Id = string;

export interface Cursor {
  generation: number;
  seq: number;
}

export interface Project {
  id: Id;
  name: string;
  path: string;
}

export interface Workspace {
  id: Id;
  projectId: Id;
  title: string;
  isolation: "local" | "worktree";
  /**
   * Checkout directory, display form. Display-only and lossy in the same
   * sense as `Session.cwd`: never compare it, never key on it, never send it
   * back. The daemon resolves directories from `id`.
   */
  path: string;
}

/**
 * The six words `git status --porcelain=v2` maps a file to, for the Changes
 * panel. `conflicted` comes from the record kind (`u`), `untracked` from `?`;
 * the other four are derived from the `XY` pair of the `1`/`2` records.
 */
export type WorkspaceGitFileStatus =
  | "modified"
  | "added"
  | "deleted"
  | "renamed"
  | "untracked"
  | "conflicted";

export interface WorkspaceGitTotals {
  additions: number;
  deletions: number;
}

export interface WorkspaceGitRow {
  /** Repository-relative path, exactly as git printed it. */
  path: string;
  /**
   * The original path of a rename row (`-z`'s bare token after the `2`
   * record); absent on every other row. A renamed row acts on **both** of
   * its paths — only the new path would leave the old side's deletion
   * staged, a half operation that answers success.
   */
  renamedFrom?: string | null;
  additions: number;
  deletions: number;
  status: WorkspaceGitFileStatus;
  /**
   * `true` when the two counts are NOT the file's exact line counts. Set by
   * every path that can make them inexact: the untracked reader refused the
   * file (over the byte cap, unreadable or gone) or stopped inside it; the
   * file carries a NUL byte; git printed `-` for it; the path is unmerged, so
   * git's numstat is stage bookkeeping rather than a delta; or the whole
   * numstat round was degraded (a dump failed or was cut), in which case
   * every number that came from it is a floor. An untracked row counts its
   * own file and is unaffected by a degraded round. Never silently short.
   */
  capped: boolean;
}

/**
 * The uncommitted working-tree state of one workspace, the reply of
 * `workspace_git_status`.
 *
 * `isGit` and `error` answer different questions and must not collapse: a
 * folder that is not a repository is `isGit: false` with `error: null`, while
 * an `error` says THIS reply is incomplete — the folder is gone, git did not
 * run, or `git status` produced more bytes than the daemon's reply cap and the
 * row list was withheld rather than cut short.
 */
export interface WorkspaceGitStatus {
  isGit: boolean;
  /**
   * Normally `!rows.length`, so the two agree. The one deliberate exception
   * is the withheld list: `git status` passed the reply cap, so `rows` is
   * empty while `dirty` still says the tree is dirty — a cut-short list is
   * not an empty tree.
   */
  dirty: boolean;
  /** `# branch.head` verbatim, including git's own `(detached)`. */
  branch: string | null;
  totals: WorkspaceGitTotals;
  rows: WorkspaceGitRow[];
  error: string | null;
}

/** Why a file diff does or does not carry lines. `ok` and `binary` are
 * complete answers; `too_large` says the lines exist and were withheld
 * rather than cut short (the sentence in `error` names which cap); `error`
 * is a refusal to answer at all. */
export type WorkspaceGitDiffStatus = "ok" | "binary" | "too_large" | "error";

/** One line's role: line-level, never word-level — `header` is a hunk
 * header (`@@ …`) kept whole, the other three are file content with their
 * `+`/`-`/space marker already stripped. */
export type WorkspaceGitDiffLineKind = "add" | "remove" | "context" | "header";

export interface WorkspaceGitDiffLine {
  kind: WorkspaceGitDiffLineKind;
  /** Without the marker for content lines; the whole `@@ …` for a header. */
  text: string;
}

/**
 * The uncommitted diff of one workspace file, the reply of
 * `workspace_git_diff` — same pair discipline as `WorkspaceGitStatus`:
 * `binary` and `too_large` are complete answers about a file deliberately
 * carried without lines, while `status: "error"` with its `error` sentence
 * is a refusal. An unchanged file is `ok` with no lines.
 */
export interface WorkspaceGitFileDiff {
  /** The path this reply is about, echoed verbatim — the caller's own text,
   * even in a refusal that rejects it; the daemon never substitutes a path
   * of its own. */
  path: string;
  /** Not in `HEAD` — untracked, staged new, or the surviving side of a
   * rename. Carve-out, the same one `additions` has: `false` whenever
   * `status !== "ok"` — a `binary`, `too_large` or `error` reply zeroes
   * the flags with the lines, and about such a reply the flags claim
   * nothing. */
  isNew: boolean;
  /** In `HEAD` and gone from the working tree, from git's `deleted file mode`.
   * Carve-out, the same one `additions` has: `false` whenever
   * `status !== "ok"` — a `binary`, `too_large` or `error` reply zeroes
   * the flags with the lines, and about such a reply the flags claim
   * nothing. */
  isDeleted: boolean;
  /**
   * Added and removed lines of `lines`. `0` whenever `status !== "ok"`:
   * a count of lines this reply does not carry would be a guess.
   */
  additions: number;
  deletions: number;
  lines: WorkspaceGitDiffLine[];
  status: WorkspaceGitDiffStatus;
  /** Why no lines came back, in one synthetic sentence: no absolute path,
   * no git stderr. `null` exactly when `status` is `ok` or `binary`. */
  error: string | null;
}

/** What a folder entry is, decided without following it: a link is never an
 * entry — the daemon skips it rather than classifying its target. */
export type WorkspaceFileKind = "dir" | "file";

export interface WorkspaceFileEntry {
  /** Path relative to the workspace folder, `/`-separated — the key the
   * tree expands and collapses by. */
  path: string;
  /** The entry's own name, for the row's label. */
  name: string;
  kind: WorkspaceFileKind;
  /** File size in bytes as `stat` reported it; `null` for a folder and
   * never a guess. */
  size: number | null;
}

/**
 * The entries of one workspace folder, the reply of
 * `workspace_files_list` — same pair discipline as `WorkspaceGitStatus`:
 * `entries` with `error: null` is the answer (an empty list is a folder
 * that holds nothing), while a sentence in `error` is a refusal and carries
 * no entries — the panel may then claim nothing about the folder behind it.
 * One directory per reply, never a subtree.
 */
export interface WorkspaceDirectory {
  /** The requested path, echoed verbatim — the caller's own text, even in a
   * refusal that rejects it; the empty string is the folder itself. */
  path: string;
  /** Already ordered by the daemon: folders first, then by name in byte
   * order. The panel renders this order and sorts nothing. */
  entries: WorkspaceFileEntry[];
  /** `true` when the entry cap dropped entries of this folder. Never
   * silently truncated. */
  capped: boolean;
  /**
   * Entries this directory had that the reply does **not** carry because
   * they failed the survival test — a link (never classified; its target is
   * not read) or an entry that would not stat. The panel shows this when it
   * is > 0: a folder with a link inside says so instead of looking
   * complete. `.git` is not in this count (declared policy exclusion), and
   * entries past `capped` belong to `capped`, not here. Never omitted.
   */
  skipped: number;
  /** Why no entries came back, in one synthetic sentence: no absolute
   * path, no OS error text. `null` exactly when the reply is an answer. */
  error: string | null;
}

/** What one file-content reply says happened — four words that never
 * collapse: `binary` and `too_large` are complete answers about a file
 * deliberately carried without content, `refused` with an `error` sentence
 * is a failure, `ok` carries the bytes. */
export type WorkspaceFileContentStatus = "ok" | "too_large" | "binary" | "refused";

/** How to read `content`: UTF-8 text, base64 for an image recognized by its
 * extension, or the bytes' own class beside a `binary` status. `null` when
 * the bytes were never read (over cap, refused). */
export type WorkspaceFileContentKind = "text" | "image" | "binary";

/**
 * The content of one workspace file, the reply of `workspace_file_read` —
 * same pair discipline as `WorkspaceDirectory`: `binary` and `too_large`
 * (the sentence carries the 128 KiB measure) are complete answers without
 * content, a sentence in `error` is a refusal, and `ok` carries the bytes —
 * base64 when `kind` is `image`, UTF-8 otherwise. `size` (bytes) and
 * `modifiedAt` (ms since the epoch) come from the stat and are `null`
 * exactly on a refusal, which claims nothing about the file it rejected.
 * The panel knows which path it asked for — the row it clicked — so the
 * reply echoes none. The five window fields below make `ok` text a
 * *window* of the file: where it starts, how many lines it holds, whether
 * another follows, and — when a line is bigger than one window — the cut
 * and the sentence that says the rest of it cannot be read this way.
 */
export interface WorkspaceFileContent {
  status: WorkspaceFileContentStatus;
  kind: WorkspaceFileContentKind | null;
  content: string | null;
  size: number | null;
  modifiedAt: number | null;
  error: string | null;
  /** The 1-based line this window starts at — `null` unless the reply is
   * `ok` text (an image's base64 has no lines; a withholding, a binary
   * and a refusal carry no window). */
  fromLine: number | null;
  /** How many lines this window holds: 0 is one addressed past the file's
   * end, and never the file's own line total — no reply counts that. */
  lines: number | null;
  /** Whether another window follows; `false` on the last one. */
  hasMore: boolean | null;
  /** Whether the byte cap cut this window's last line short — declared by
   * the reply, never left for the panel to guess. */
  truncated: boolean | null;
  /** The sentence that cut carries: `null` unless `truncated` is true, and
   * static words (never a path) when it is — the wire's own admission that
   * the line exceeds one window and the rest of it cannot be read this
   * way. */
  note: string | null;
}

/**
 * The outcome of one workspace write — a rename or a duplicate, the reply
 * of `workspace_file_rename` / `workspace_file_duplicate` — same pair
 * discipline as `WorkspaceDirectory`: exactly one field is non-null. A
 * success carries `newPath` — the entry's new spelling, `/`-joined the way
 * the tree's own paths are, so it can be keyed, selected and re-read like
 * any path a listing handed back — and `error: null`; a refusal carries a
 * synthetic sentence (no absolute path, no OS error text) and
 * `newPath: null`, and the panel claims nothing about where anything is.
 */
export interface WorkspaceFileMutation {
  newPath: string | null;
  error: string | null;
}

/**
 * How the Files panel draws a staged preview — decided from the spelling
 * alone, the way the daemon decides what it will stage at all: the three
 * words here and the daemon's three lists (`IMAGE_EXTENSIONS` in
 * `workspace_file_read.rs`, `VIDEO_EXTENSIONS` + `pdf` in
 * `workspace_file_preview.rs`) are mirrors, and an extension joins both or
 * neither — a panel that staged something the daemon refuses would show
 * the wire's sentence, which is safe but not a preview.
 */
export type PreviewMediaKind = "image" | "video" | "pdf";

/**
 * The reply of `workspace_file_preview_stage` — the daemon's answer before
 * the app turns its path into an asset URL. A discriminated pair with the
 * same discipline as `WorkspaceFileContent`'s invariants, checkable by the
 * compiler: `ok` carries the absolute path of the copy (inside the
 * daemon's `previews` folder, the only folder the asset protocol
 * concedes) beside the source file's own stat — `size` is bytes,
 * `modifiedAt` milliseconds since the epoch, each `null` only when the
 * filesystem gave no number; `refused` carries the sentence and claims
 * nothing: no path, no stat, no copy.
 */
export type WorkspaceFilePreview =
  | {
      status: "ok";
      path: string;
      size: number | null;
      modifiedAt: number | null;
      error: null;
    }
  | { status: "refused"; path: null; size: null; modifiedAt: null; error: string };

/**
 * What the panel holds after a stage: the copy's asset URL — built from
 * the daemon's path with Tauri's own `convertFileSrc` inside
 * `workspaceFilePreviewStage`, so the URL is the only form of the copy
 * that ever reaches this module — plus the media kind the panel draws it
 * as and the source stat for the header; or the refusal's sentence.
 * Unstage has its own reply shape: it answers `void`, because the only
 * path worth naming is one that must stop existing.
 */
export type WorkspaceFileStaged =
  | {
      status: "ok";
      url: string;
      kind: PreviewMediaKind;
      size: number | null;
      modifiedAt: number | null;
    }
  | { status: "refused"; error: string };

export type SessionKind = "terminal" | "acp" | "claude" | "pi" | "codex";

export function isAgentKind(kind: SessionKind): kind is "acp" | "claude" | "pi" | "codex" {
  return kind === "acp" || kind === "claude" || kind === "pi" || kind === "codex";
}
export type SendIntent = "interrupt" | "steer" | "queue";

/**
 * What a `session_send` does when the target session already has a turn
 * running: the protocol's `SessionSend.activeTurnBehavior`. Omitting the field
 * is the daemon's default — interrupt the running turn and replace it — so the
 * only member here is the one value that differs from it: `"steer"` delivers
 * the text into the running turn. The protocol's third word, `"interrupt"`, is
 * never sent and is expressed by leaving the field off; the wider `SendIntent`
 * above spells all three and has no caller.
 *
 * `"queue"` is deliberately absent: no daemon branch implements it, and a
 * value the daemon silently treats as interrupt-and-replace would be a type
 * that promises behaviour nothing delivers. It can come back with the branch.
 */
export type ActiveTurnBehavior = "steer";

/**
 * Who authored one `agent_user_message` echo (protocol `UserMessageAuthor`).
 * Not session origin nor envelope role: whose words the echo carries.
 */
export type UserMessageAuthor = "human" | "agent" | "creation";

/** What an `agent_user_message` means in the session displaying it. */
// Keep aligned with protocol::UserMessageKind; the protocol crate tests the
// serde names against this union.
export type UserMessageKind =
  | "unknown"
  | "composer"
  | "outgoing_a2a"
  | "incoming_a2a"
  | "system_notice"
  | "creation";

export type PermissionOutcome = "allow_once" | "deny";

/**
 * Which device a session belongs to, in the protocol's own words.
 *
 * `unknown` is the daemon's own word for a journal row whose origin column it
 * could not interpret. It is provenance the app does not have, and it is not a
 * synonym for `local`: both the card and the tab render it exactly as they
 * render an absent origin.
 */
export type SessionOriginKind = "local" | "peer" | "unknown";

/**
 * Where a session came from: this machine, or a paired peer (protocol
 * `SessionOrigin`). `deviceId` is the key the session badge resolves to a
 * display name; `role` says which grade of device it is, in `PeerRole`'s own
 * vocabulary. Both are absent on a local origin.
 *
 * The value as a whole may be absent, which is a third state and not a local
 * one: the daemon now stamps an origin on every session, so a missing one only
 * comes from a daemon older than the field. It renders as unknown — see
 * `Session.origin`. A `kind` of `unknown` is a fourth state and renders exactly
 * as the absent one does: as provenance the app does not have.
 */
export interface SessionOrigin {
  kind: SessionOriginKind;
  deviceId?: string;
  role?: PeerRole;
}

export interface PermissionOption {
  optionId: string;
  name: string;
  kind: string;
}

export interface ToolLocation {
  path: string;
  line?: number;
}

export interface PermissionEnvVar {
  name: string;
  value: string;
}

export interface PermissionRequest {
  type: "permission_request";
  toolCallId: Id;
  title: string;
  description?: string;
  command?: string;
  args?: string[];
  cwd?: string;
  env?: PermissionEnvVar[];
  options: PermissionOption[];
  /**
   * The daemon's chooser verdict: present and `true` exactly when the
   * option set trips Paseo's rule (the same allow kind offered twice means
   * the agent is asking which one to use). The card renders one control per
   * option when it is set; absent — an ordinary permission, or a frame from
   * a daemon older than this field — renders the ordinary Allow once and
   * Deny pair. The app reads this mark and never re-derives the rule from
   * the option list.
   */
  isChooser?: boolean;
  /**
   * The origin of the session this request belongs to, when the daemon sends
   * it. The card renders a `peer` origin as its own provenance line, in its own
   * element: the request's own text must never be able to imitate it. A `local`
   * origin renders no line at all, and an absent one — only an older daemon
   * sends that — renders `Origin: unknown` in that same element, because
   * staying silent would make it read exactly like a local request. A `kind`
   * of `unknown` renders that same line: it is a request the app cannot place,
   * not a local one.
   */
  origin?: SessionOrigin;
  /**
   * Present only on a **creation card** — the card an agent's
   * `devboule_create_agent` call raises. The ordinary card fields say what is
   * being asked and `options` carries allow-once/deny, exactly as for any other
   * permission; this field says what would be created. The daemon publishes it
   * through the same broker entry, so answering it is the same
   * `sessionPermissionRespond` call and a refusal leaves the gate shut.
   */
  createAgent?: CreateAgentCard;
}

/** The caps one creation is admitted under, as its card states them. */
export interface CreateAgentCaps {
  liveChildren: number;
  maxLiveChildren: number;
  creationsThisHour: number;
  maxCreationsPerHour: number;
  depth: number;
  maxDepth: number;
  liveAgentSessions: number;
  maxLiveAgentSessions: number;
}

/** What a creation card is asking the human to authorize. */
export interface CreateAgentCard {
  creatorSessionId: Id;
  provider: string;
  /**
   * The **name** of the profile the child would be created from: the word the
   * human ticked. What it resolves to (model, mode, features) is on the card's
   * description; the child's session row records the profile's stable id
   * (`Session.profileId`), never this name.
   */
  profile: string;
  /** The display name the child would be created with. */
  title: string;
  /**
   * The tools state the child will start in (`hosted`/`unavailable`/`unverified`).
   * The card promises verification; the result and roster report it.
   */
  tools: string;
  caps: CreateAgentCaps;
}

/**
 * The A2A `TaskState` vocabulary (`completed | failed | canceled` are the
 * terminal three a finish report uses).
 */
export type AgentTaskState =
  | "submitted"
  | "working"
  | "completed"
  | "failed"
  | "canceled"
  | "input_required"
  | "rejected";

/** One part of a finish artifact. `url` is a reference, never a path. */
export interface FinishArtifactPart {
  url: string;
  mimeType: string;
  metadata?: { storedBytes: number };
}

/** One artifact a child's finish deposited: its whole last message. */
export interface FinishArtifact {
  artifactId: string;
  parts: FinishArtifactPart[];
}

export interface PermissionResolved {
  type: "permission_resolved";
  toolCallId: Id;
  selectedOptionId?: string;
  selectedOptionKind?: string;
  selectedOptionName?: string;
  /**
   * Who answered the card, when the daemon says so: the session id of the
   * agent that answered through delegated permission answering — the same
   * string the daemon journals in `PermissionAnswered.answered_by`. `null` and
   * absent mean the daemon DID NOT SAY who answered — they are not an
   * attribution, and they never mean "a person answered": reading the wire's
   * silence as a named human is exactly the collapse this field exists to
   * prevent. The card renders an unnamed-answer state for them, and the
   * outcome word (`selectedOptionKind`) is attributed separately from the
   * answerer.
   */
  answeredBy?: Id | null;
}

/**
 * The durable record of a resolution, on every resolution — a person's, a
 * delegated one, an auto-answer and a cancel alike. `answeredBy` is absent
 * for a human and names the creator session for a delegated answer.
 */
export interface PermissionAnswered {
  type: "permission_answered";
  cardId: Id;
  answeredBy?: Id | null;
  outcome: string;
}

export interface SessionModelEffort {
  id: string;
  label: string;
  description?: string;
  default?: boolean;
}

export interface SessionModel {
  modelId: string;
  name: string;
  description?: string;
  contextTokens?: number;
  currentEffort?: string;
  efforts?: SessionModelEffort[];
}

export interface SessionModeView {
  id: string;
  name: string;
  description?: string;
}

export interface SessionModeState {
  currentModeId: string;
  availableModes: SessionModeView[];
}

export interface SessionManifest {
  type: "session_manifest";
  providerId?: string;
  currentModelId?: string;
  models: SessionModel[];
  modes?: SessionModeState;
}

/** One rate-limit window a plan-usage frame actually carried (protocol
 * `PlanWindow`). `durationMins` labels the window — Codex sends 300 for the
 * 5-hour window and 10080 for the weekly one. `resetsAt` is Unix seconds. */
export interface PlanWindow {
  durationMins: number;
  /** Absent when the frame named the window but not its consumption — the
   * popover then shows no percent, never a stand-in 0. */
  usedPercent?: number;
  resetsAt?: number;
}

/** The credits block of a plan-usage frame, when the frame had one
 * (protocol `PlanCredits`). */
export interface PlanCredits {
  /** The balance exactly as the provider spelled it, when it sent one. */
  balance?: string;
  /** Whether the balance is unlimited; absent when the frame did not say —
   * an absent field is never rendered as `false`. */
  unlimited?: boolean;
}

export type NoticeSeverity = "info" | "warning";

export interface SessionNotice {
  type: "session_notice";
  text: string;
  severity: NoticeSeverity;
}

export type RetentionSource = "default" | "user";

export interface RetentionLimit {
  value: number;
  source: RetentionSource;
}

export interface JournalRetention {
  sessionMaxBytes: RetentionLimit;
  maxBytes: RetentionLimit;
  maxSessions: RetentionLimit;
  maxAgeMs: RetentionLimit;
}

export interface RetentionPatch {
  sessionMaxBytes?: number;
  maxBytes?: number;
  maxSessions?: number;
  maxAgeMs?: number;
}

export interface JournalLimits {
  snapshotEveryBytes: number;
  sessionMaxBytes: number;
  maxBytes: number;
  maxSessions: number;
  maxAgeMs: number;
}

export interface JournalSessionUsage {
  id: Id;
  title: string;
  /**
   * The name a created agent is shown under, when the row has one (protocol
   * `JournalSessionUsage.displayName`, which is the journal's own
   * `display_name` column). Absent means the session has no name of its own:
   * History renders the fallback name, exactly as the tab strip does — never an
   * empty label.
   */
  displayName?: string;
  kind: SessionKind;
  bytes: number;
  updatedAtMs: number;
}

export interface Unreclaimable {
  bytesOver: number;
  sessionsOver: number;
  agedOut: number;
}

export interface JournalUsage {
  totalBytes: number;
  sessionCount: number;
  deletedByUser: number;
  deletedByRetention: number;
  unreclaimable: Unreclaimable;
  limits: JournalLimits;
  perSession: JournalSessionUsage[];
}

export type TranscriptIntegrity =
  | { kind: "complete" }
  | {
      kind: "truncated";
      droppedFrames: number;
      droppedBytes: number;
      trimmedBytes: number;
    }
  | {
      kind: "unverifiable";
      droppedFrames: number;
      droppedBytes: number;
      trimmedBytes: number;
    };

export type UnverifiableTranscriptIntegrity = Extract<
  TranscriptIntegrity,
  { kind: "unverifiable" }
>;

export type SessionState =
  | { type: "live"; generation: number }
  | { type: "silent"; generation: number }
  | {
      type: "ended";
      generation: number;
      code: number | null;
      integrity: TranscriptIntegrity;
    }
  | {
      type: "recovered";
      generation: number;
      integrity: UnverifiableTranscriptIntegrity;
    };

/**
 * One file attached to a prompt, in the shape the daemon's `PromptAttachment`
 * deserializes.
 *
 * `data` is the bytes, base64, and never a path: the daemon that talks to the
 * provider writes the file itself, so the same message can be forwarded to
 * another device unchanged. `name` is display metadata — the daemon digests the
 * bytes and never puts this name in a path.
 */
export interface PromptAttachment {
  name: string;
  /**
   * The types the daemon accepts. `text/markdown` is a *deposit* type only:
   * the finish report stores a child's last message under it (`S5` decision
   * 10). A composer sends the image types it can preview.
   */
  mimeType: "image/png" | "image/jpeg" | "image/svg+xml" | "text/markdown";
  data: string;
}

/**
 * Whether a session can pass a permission moment with no human answering (the
 * closed wire enum the daemon derives from the session's **delivered** mode).
 * A mode whose vocabulary is the provider's own — an ACP agent's modes are
 * three free strings, prose the daemon did not author — is `"unknown"`: the
 * daemon was not told, and reading that as `"no"` would claim a human is
 * watching when nobody knows. `"unknown"` renders as its own present marker,
 * never as nothing and never as `"no"`.
 */
export type UnattendedState = "yes" | "no" | "unknown";

export interface Session {
  id: Id;
  workspaceId: Id | null;
  kind: SessionKind;
  title: string;
  provider?: string;
  peerSessionId?: string;
  state: SessionState;
  /** Milliseconds since the last observed output; null for recovered records. */
  elapsedMs: number | null;
  /**
   * The directory the process was actually given, echoed back by the daemon in
   * display form. A live session reports it from the command it launched; a
   * journal-only transcript reports the directory its birth recorded (journal
   * schema v15) and is absent only for a row that predates that column or was
   * never launched. The frontend must render it or say nothing; it must never
   * substitute a guess, and it must never send a cwd of its own: the daemon
   * resolves the path from `workspaceId`, or from the row's own record.
   */
  cwd?: string;
  /**
   * Unix milliseconds when the session was first created, stable across
   * resume. Pair it with `id` to tell "my saved id still means this session"
   * from "same id, different session": ids embed a per-daemon-process
   * random nonce beside the counter (`session_unique`), so two lives of the
   * daemon cannot mint the same id — but resume deliberately reuses both the
   * id and this stamp, which is what makes the pair an identity check rather
   * than a timestamp. A row sharing only the id never matches.
   *
   * Optional here and never optional on the wire: every session from
   * `sessionsList` has it. It is absent only on a session this frontend
   * synthesized from a roster push (`sessions_watch`) for an id it had not
   * listed yet, because the tab strip does not need creation time. The
   * snapshot carries workspace and kind identity separately. Absent therefore
   * means "this row came from a roster push", never "the creation time is
   * unknown" — do not fill it in.
   */
  createdAtMs?: number;
  /** Mirror of the roster snapshot's attention; the frontend only renders it. */
  attention?: Attention;
  /**
   * Where the session came from. Absent means the daemon did not say — a
   * record written before the field existed, or a roster push that omitted it
   * for a row no list response has described yet. Absent is a third state, not
   * a local one: the tab shows an `origin unknown` badge rather than staying
   * silent and looking like a local session.
   */
  origin?: SessionOrigin;
  /**
   * The name a created agent is shown under (protocol `Session.displayName`).
   * Set once at creation and never renamable, so a row that has one always had
   * it. Absent means the session has no name of its own: render the fallback,
   * never an empty label. A human-started session may carry one too — the same
   * field, chosen by whoever created it.
   */
  displayName?: string;
  /**
   * The session id of the agent that created this one (protocol
   * `Session.createdBy`). Daemon-written only: the wire's create frame has no
   * such field, so a peer cannot claim a parent. Absent means a human started
   * the session, or the row predates the field.
   */
  createdBy?: string;
  /**
   * The profile this session was created from, by its **stable id** (protocol
   * `Session.profileId`). A rename of that profile later leaves this alone, so a
   * running child never misreports what it was started from; resolve the id
   * against the stored document for a name to show. Absent for a session a human
   * started from the provider picker, and for older rows.
   */
  profileId?: string;
  /**
   * The context this session belongs to: its own id, unless another session
   * created it, in which case it is that creator's context (protocol
   * `Session.contextId`). One value for a creator and everything it commissions,
   * at any depth.
   */
  contextId?: Id;
  /**
   * Whether this session can pass a permission moment with no person
   * answering — a fact of its birth, from the profile that created it
   * (`DESIGN-what-unattended-means.md`). The three values are distinct on
   * purpose and never collapse: `yes` — the daemon knows the delivered mode
   * asks nobody; `no` — it knows the mode asks; `unknown` — the mode's
   * vocabulary is the agent's own prose, and deriving a permission fact from
   * prose is a defect, so the daemon says so instead of guessing. It replaces
   * the collapsed boolean the audit found (`bool` + `#[serde(default)]`
   * reporting "asks" about a child that asks nobody): `unknown` must render
   * as its own, present marker — never as `no`, never as nothing.
   *
   * Absent means the daemon has not said — a session a person started, or a
   * row older than the field. Absent is not `no`, and the row renders nothing
   * for it rather than guessing.
   */
  unattended?: UnattendedState;
  /**
   * The session's labels: the caller's own map plus the four `devboule.` keys
   * the daemon stamps. For display, and for nothing else — no code decides
   * anything from a label.
   */
  labels?: Record<string, string>;
  /**
   * Whether the daemon would accept a resume for this session right now:
   * process gone, resumable family, provider and peer id persisted
   * (protocol `Session.resumable`, computed from `Provider::resumable()`).
   * The panel renders it and never re-derives it from kind, state, or
   * columns. Absent means the daemon predates the field — render no Reopen
   * button rather than guessing from the kind.
   */
  resumable?: boolean;
  /**
   * Mirror of the roster snapshot's `delegation`; the frontend only renders
   * it. The wire's `Session` (what `sessions_list` answers) never carries the
   * field — it is written onto this record only by the roster push merge
   * (`applySnapshot` in `workspaceSessions.ts`), so absent here means "no push
   * has described this row yet" as well as "not an agent-created child", and
   * the row renders nothing for it either way. It is never read as "delegation
   * is off".
   */
  delegation?: DelegationState;
}

export type ResumeResult =
  | { type: "resumed"; session: Session }
  | { type: "not_supported" }
  | { type: "failed"; message: string };

/** Journal writer counters nested inside the daemon's health section. */
export interface JournalStatsDiagnostics {
  acceptedFrames: number;
  acceptedBytes: number;
  committedFrames: number;
  committedBytes: number;
  failedFrames: number;
}

/**
 * Diagnostics report from the daemon, already redacted server-side. The
 * frontend renders exactly what it is given and never sanitises it. This is a
 * direct camelCase mirror of `DiagnosticsReport`; journal facts are nested in
 * `health` on the wire and are presented as a derived Journal section by the
 * panel.
 */
export interface DaemonDiagnostics {
  /** Identity and version. */
  daemon: {
    version: string;
    protocolVersion: number;
    pid: number;
    uptimeMs: number;
    clients: number;
    sessions: number;
    capabilities: string[];
    instanceId: string;
  };
  /** Health counters and nested journal facts. */
  health: {
    peakRingBytes: number;
    ringEvictedBytes: number;
    ringDroppedFrames: number;
    journalStats: JournalStatsDiagnostics | null;
    journalError?: string;
    journalSchemaVersion: number;
    journalFileBytes?: number;
  };
  /** Aggregate session counts. `oldestLiveAgeMs` is null when no session is live. */
  sessions: {
    total: number;
    live: number;
    silent: number;
    ended: number;
    recovered: number;
    terminal: number;
    acp: number;
    claude: number;
    pi: number;
    codex: number;
    resumable: number;
    oldestLiveAgeMs: number | null;
  };
  /** One row per provider, redacted server-side. */
  providers: Array<{
    id: string;
    protocol: string | null;
    origin: string | null;
    installChannel: string | null;
    installedVersion: string | null;
    latestVersion: string | null;
    agentVersion: string | null;
    installed: boolean;
    authentication: string;
  }>;
  /** Environment facts. */
  environment: {
    osVersion: string;
    appVersion: string;
    runtimeDir: string;
    pipeName: string;
    loginShellCapture: {
      state: "not_run" | "applied" | "skipped" | "failed";
      appliedVariables: number;
      preservedVariables: number;
    };
  };
}

/** Why a session wants the user's attention. Suppression is daemon-side policy. */
export type AttentionReason = "finished" | "error" | "permission";

export interface Attention {
  reason: AttentionReason;
  atMs: number;
}

/** Compact daemon push used to update the workspace tab roster. */
export interface SessionStateSnapshot {
  id: Id;
  workspaceId: Id | null;
  kind: SessionKind;
  title: string;
  state: SessionState;
  elapsedMs: number | null;
  /** Absent when the session needs no attention; suppression is daemon-side. */
  attention?: Attention;
  /**
   * The session's origin, when the daemon carries it on the push. A push that
   * omits it leaves a row already listed by `sessionsList` its known origin; a
   * row no list has described keeps none, which renders as unknown, never local.
   */
  origin?: SessionOrigin;
  /**
   * The name a created agent is shown under, when the daemon carries it on the
   * push. A push that omits it leaves a row already described by `sessionsList`
   * its known name; a row no list has described keeps none, which renders as the
   * row's fallback name, never as an empty one.
   */
  displayName?: string;
  /**
   * The session that created this one, when the daemon carries it on the push.
   * Same rule as `displayName`: absent means "no list has said yet", and the row
   * shows no created-by badge rather than guessing one.
   */
  createdBy?: Id;
  /**
   * The profile this session was created from, by its stable id. Carried on
   * every push for the same reason as `displayName`: a child created while the
   * app is open arrives as a push-only row.
   */
  profileId?: string;
  /**
   * The context this session belongs to (its own id, or its creator's). Carried
   * on every push, like the name and the creator.
   */
  contextId?: Id;
  /**
   * The unattended marker, in the same three values as `Session.unattended`,
   * carried on every push for a child. Same absent rule: absent means the
   * daemon has not said, never `no`.
   */
  unattended?: UnattendedState;
  /** The session's labels, stamped by the daemon. */
  labels?: Record<string, string>;
  /**
   * The delegation ledger for one agent-created child, carried on every push
   * so the row's answered count never goes stale behind a cached one.
   *
   * **Absent means this session is not an agent-created child — never
   * "delegation is off."** A badge rendered for an absent field is the
   * absent-into-none collapse wearing a roster badge: a human-started session
   * would read as a child nobody answers for. Present, `state` says who
   * answers the child's permission cards: `"active"` — its creator does while
   * the setting is on; `"off"` — nobody does; `"unattended"` — the child was
   * created in an auto-accepting profile and asks nobody at all, which is a
   * fact of its birth and outlives every later flip of the setting (the daemon
   * reads it from its journal, never from the live setting). `answered` counts
   * the child's cards answered by anyone, human answers included; attribution
   * lives on the card, the count on the row.
   *
   * When a push violates that contract — it omits the ledger for a row the app
   * already knows is a child — the app does not render the benign absence: the
   * merge in `applySnapshot` carries the last described ledger, or mints
   * `state: "unknown"` for a child never described. See `workspaceSessions.ts`.
   */
  delegation?: DelegationState;
}

/**
 * Whether an agent-created child asks a person before it acts. Daemon-side it
 * is the roster's `Option<DelegationState>`; the four render cases live in
 * `sessionDelegationBadge` (`workspaceSessions.ts`).
 */
export interface DelegationState {
  /** Cards of this child answered by anyone — human answers included. */
  answered: number;
  /**
   * Closed on the daemon's three states plus one app-minted sentinel:
   * `"unknown"` is never sent by the daemon — the app mints it when a child
   * the roster already knows is left undescribed by a push, or when a push
   * carries a state value this build cannot read. It renders as its own,
   * present marker; it must never collapse into `"off"`, which is the benign
   * case it most resembles. See `sessionDelegationBadges`.
   */
  state: "off" | "active" | "unattended" | "unknown";
}

/**
 * Where the delegation switch's stored answer came from, in the daemon's own
 * words for the three cases. They are three different facts and the panel must
 * keep them three sentences: `file` — a human wrote `delegation.json`, so the
 * value is deliberate; `default` — no file exists yet, which reads "never
 * configured", not "off"; `quarantined` — the file existed and was damaged, so
 * the daemon quarantined it and reads off. A damaged file is neither
 * never-configured nor deliberately off, and reporting either is a lie.
 */
export type DelegationSourceState = "file" | "default" | "quarantined";

/** The `delegation_get` reply: the stored answer plus where it came from. */
export interface DelegationReply {
  enabled: boolean;
  source: DelegationSourceState;
}

export type CursorShape = "block" | "underline" | "bar";

export interface ScreenCursor {
  row: number;
  col: number;
  visible: boolean;
  shape: CursorShape;
  blinking: boolean;
}

export interface SessionSnapshot {
  type: "snapshot";
  asOfSeq: number;
  cols: number;
  rows: number;
  data: string;
  cursor: ScreenCursor;
  alternateScreen: boolean;
  bracketedPaste: boolean;
  lineWrap: boolean;
  title?: string;
}

/**
 * Attachment events plus the connection-scoped roster snapshot.
 * Alignment with `SessionEvent` in `crates/devboule-protocol` is enforced by
 * `session_event_guard` (committed snapshot + union parse) and by the
 * handler coverage test in `terminalSession.test.ts`.
 */
export type SessionEvent =
  | { type: "output"; seq: number; data: string }
  | SessionNotice
  /** Text emitted by an ACP agent message chunk. */
  | {
      type: "agent_message";
      messageId: string | null;
      text: string;
      parentToolUseId?: string;
      spawnDepth?: number;
    }
  /** ACP tool call announced by the agent. */
  | {
      type: "agent_tool_call";
      toolCallId: string;
      title: string;
      status: string;
      kind?: string;
      locations?: ToolLocation[];
      subagentType?: string;
      parentToolUseId?: string;
      spawnDepth?: number;
    }
  /** ACP update for an existing tool call. */
  | {
      type: "agent_tool_update";
      toolCallId: string;
      status: string | null;
      text: string | null;
      title?: string;
      kind?: string;
      locations?: ToolLocation[];
      parentToolUseId?: string;
      spawnDepth?: number;
    }
  /**
   * ACP prompt completion. `modelId` and `usage` are what the agent actually
   * ran and spent, when it says so; grok reports them, others may not.
   */
  | {
      type: "agent_finished";
      stopReason: string;
      modelId?: string;
      usage?: {
        inputTokens?: number;
        outputTokens?: number;
        totalTokens?: number;
        thoughtTokens?: number;
      };
    }
  /**
   * How full the context window is, in the provider's own words (protocol
   * `SessionEvent::ContextUsage`). `live` is false when the number is the end
   * of the last turn rather than a mid-turn push. `maxTokens` is absent when
   * no frame carried the window; the app may then read the window from the
   * manifest entry for the SAME `modelId`, never from another model.
   */
  | {
      type: "context_usage";
      modelId?: string;
      usedTokens: number;
      maxTokens?: number;
      live: boolean;
    }
  /**
   * Plan windows a provider pushed on the wire (protocol
   * `SessionEvent::PlanUsage`) — today only Codex `account/rateLimits/updated`.
   * One entry per window the frame carried; a window the frame did not send
   * is never added, and a missing percent stays missing rather than becoming
   * zero. Account-scoped: the app keeps the latest event per `providerId`.
   */
  | {
      type: "plan_usage";
      providerId: string;
      planLabel?: string;
      windows: PlanWindow[];
      credits?: PlanCredits;
    }
  /**
   * Echo of the user prompt, one ACP `user_message_chunk` at a time.
   *
   * `author` names who spoke. `messageKind` separately names the part this
   * text plays in the displaying session; older journal rows omit it and use
   * the legacy envelope classifier in the webview.
   */
  | {
      type: "agent_user_message";
      messageId: string | null;
      text: string;
      author: UserMessageAuthor;
      /** Optional for frames written before this field existed. */
      messageKind?: UserMessageKind;
    }
  /**
   * An agent created a child session (protocol `SessionEvent::AgentCreated`).
   * Published on the **creator's** transcript, never on the child's, so the
   * creator's record explains where the session came from.
   */
  | {
      type: "agent_created";
      messageId: string | null;
      childSessionId: Id;
      displayName: string;
      provider: string;
      /**
       * The **name** of the profile the child was created from, as it was called
       * at that moment. The child's session row carries the profile's stable id
       * (`Session.profileId`), because a rename must not make a running child
       * misreport what it was started from; this is the sentence to show.
       */
      profile: string;
    }
  /**
   * A created child finished (protocol `SessionEvent::ChildFinished`). The
   * structured twin of the `<devboule-system>` text message the daemon sends the
   * creator: same facts, same `messageId`, and the app reads THIS one — it has
   * no parser for the envelope and must not grow one.
   *
   * The app's only reader is the Design mirror, which reads the first
   * markdown part through the attachment-read door (`sessionAttachmentRead`)
   * on arrival; the history entry points at the child with a session id and
   * no url. Nothing else is copied on arrival, and the daemon's own copy is
   * the only one — it dies with the creator session, as
   * `SessionEvent::ChildFinished` says in `devboule-protocol/src/session.rs`.
   */
  | {
      type: "child_finished";
      messageId: string | null;
      childSessionId: Id;
      displayName: string;
      state: AgentTaskState;
      /** Why `artifacts` is empty, when it is. Absent with an artifact. */
      note?: string;
      artifacts: FinishArtifact[];
    }
  /**
   * The daemon accepted a prompt into a turn that was already running
   * (protocol `SessionEvent::Steered`). Journaled for audit and not emitted to
   * observers, so no view renders it, and it is listed so the protocol snapshot
   * and this union keep the same tags: what the sender sees live is the
   * `agent_user_message` echo published for the same accepted text (S4-07).
   * Rendering steered messages from this event is slice 4b (OQ-6).
   */
  | { type: "steered"; messageId: string | null; text: string }
  /** Agent reasoning, one ACP `agent_thought_chunk` at a time. */
  | {
      type: "agent_thought";
      messageId: string | null;
      text: string;
      parentToolUseId?: string;
      spawnDepth?: number;
    }
  /** Claude stream-json subagent birth. */
  | {
      type: "agent_task_started";
      taskId: string;
      title?: string;
      subagentType?: string;
      toolUseId?: string;
      isBackgrounded?: boolean;
      spawnDepth?: number;
    }
  /** Claude stream-json subagent terminal notification. */
  | {
      type: "agent_task_notification";
      taskId: string;
      toolUseId?: string;
      status: "completed" | "failed" | "stopped";
      summary?: string;
    }
  /** Replacement set of Claude background tasks. */
  | {
      type: "agent_background_tasks_changed";
      tasks: Array<{ taskId: string; taskType: string; title: string }>;
    }
  /** Slash commands the agent advertises for this session. */
  | {
      type: "available_commands";
      commands: Array<{ name: string; description: string; hint?: string }>;
    }
  /** ACP protocol or framing error surfaced by the daemon. */
  | { type: "agent_error"; message: string }
  /** One line drained from the ACP agent's stderr. */
  | { type: "agent_stderr"; data: string }
  | {
      type: "agent_reported";
      seq: number;
      source: string;
      agent: string;
      state: "idle" | "working" | "blocked" | "unknown";
      message?: string;
      reportSeq?: number;
      agentSessionId?: string;
      agentSessionPath?: string;
      sessionStartSource?: string;
    }
  | PermissionRequest
  | PermissionResolved
  | PermissionAnswered
  | SessionManifest
  | { type: "exit"; code: number | null }
  | { type: "silent"; elapsedMs: number }
  | { type: "recovered"; integrity: UnverifiableTranscriptIntegrity }
  /** Another client took the session over: this view is dead, the session is not. */
  | { type: "detached" }
  | { type: "journal_degraded"; droppedFrames: number; droppedBytes: number }
  /** Connection-scoped roster update; not an attach-channel event. */
  | { type: "sessions_snapshot"; sessions: SessionStateSnapshot[] }
  | SessionSnapshot;

/** The `context_usage` member, named for the state and components that carry it. */
export type ContextUsage = Extract<SessionEvent, { type: "context_usage" }>;

/** The `plan_usage` member, named for the per-provider store that keeps it. */
export type PlanUsage = Extract<SessionEvent, { type: "plan_usage" }>;

export type DaemonConnectionState =
  | "connected"
  | "connecting"
  | "disconnected"
  | "error"
  | "unresponsive";

export interface DaemonStatus {
  state: DaemonConnectionState;
  pid: number | null;
  instanceId: string | null;
  protocolVersion: number | null;
  clients: number | null;
  capabilities: string[];
  /** For `unresponsive`: a human sentence from the supervisor, shown verbatim. */
  message: string | null;
  /**
   * Remote reachability and the secret store the daemon selected. Both live in
   * the daemon's `Status` body; this supervisor projection does not forward
   * them yet (`UiDaemonStatus` in `src-tauri/src/client/mod.rs`), so they stay
   * optional here. The Devices panel reads the authoritative copy from
   * `DevicesReply.selfInfo.remote` instead of guessing from these.
   */
  remote?: RemoteState;
  secretStore?: SecretStore;
}

/**
 * Machine-readable failure. Matches `ErrorCode` in the protocol crate (snake_case).
 * Alignment is enforced by `error_code_matches_frontend_union` in
 * `crates/devboule-protocol/src/error.rs`.
 */
export type ErrorCode =
  | "protocol_version_mismatch"
  | "unauthorized"
  | "unimplemented"
  | "capability_not_supported"
  | "invalid_request"
  | "session_not_found"
  | "session_generation_mismatch"
  | "idempotency_conflict"
  | "shutting_down"
  | "journal"
  | "workspace_unavailable"
  | "workspace_confinement_refused"
  | "internal"
  | "io"
  | "connection_lost";

/** Matches `ErrorDetails` in the protocol crate. Field names stay snake_case. */
export type ErrorDetails =
  | {
      type: "version_mismatch";
      client: number;
      client_min: number;
      daemon: number;
      daemon_min: number;
    }
  | {
      type: "generation_mismatch";
      current: number;
      requested: number;
    };

/** Payload Tauri rejects with when a command returns `Err(CommandError)`. */
export interface CommandError {
  code: ErrorCode;
  message: string;
  details?: ErrorDetails;
}

export interface ProviderInfo {
  id: string;
  executable: string;
  acpAvailable: boolean;
  /**
   * Wire contract set by the daemon: `"unknown"` = never measured, `"ok"` =
   * most recent provider start completed, `"failed: <reason>"` = most recent
   * start failed with a one-line reason.
   */
  authentication: string;
  /** `"acp"`, `"stream-json"`, `"pi-rpc"`, or `"codex-app-server"` when chat is available. */
  protocol?: string | null;
  /** `"user-binary"` from PATH; `"npx-wrapper"` from the ACP registry. */
  origin?: "user-binary" | "npx-wrapper" | null;
  /** Registry-supplied args appended after `npx -y <package>`. */
  launchArgs?: string[] | null;
  /** Explicit picker policy; covered wrappers remain visible in Settings. */
  pickable?: boolean | null;
  /** Version of the CLI binary installed locally; null when it could not be probed. */
  installedVersion?: string | null;
  /** Newest version the daemon knows is available; null when unknown. */
  latestVersion?: string | null;
  /**
   * Version the running adapter reported during its last live ACP handshake.
   * This is the adapter's own claim and may differ from `installedVersion`.
   */
  agentVersion?: string | null;
  /** How the CLI is installed; null when the daemon could not determine it. */
  installChannel?: "npm" | "npx-registry" | "native" | null;
  /**
   * Whether the CLI is installed locally. ABSENT means installed; `false`
   * appears only on synthetic "known but not installed" rows the daemon builds
   * from its npm-package table. Those rows carry no protocol, so the chat
   * picker excludes them already. The daemon never emits null.
   */
  installed?: boolean;
  /** npm package the row maps to, when known; the update/install consent shows it verbatim. */
  npmPackage?: string | null;
  /**
   * MCP tools this provider's sessions can serve, present only for the four
   * native MCP-capable providers (`claude`, `gemini`, `grok`, `qwen`).
   * Omitted (absent key) when empty: wrappers and non-MCP providers carry
   * no tools. The panel renders its Tool settings section only when this is
   * non-empty.
   */
  tools?: ToolDescriptor[];
}

/**
 * One MCP tool a provider's sessions can serve. Mirrors `ToolDescriptor` in
 * the protocol crate (`rename_all = "camelCase"`).
 */
export interface ToolDescriptor {
  name: string;
  description: string;
}

/**
 * One stored tool-policy row, as `ToolPolicyGet` answers it. Mirrors
 * `ToolPolicyEntry` in the protocol crate (`rename_all = "camelCase"`).
 *
 * `enabled` is optional on the wire and absent means enabled: `None` or
 * `Some(true)` = enabled, `Some(false)` = all tools disabled. The panel
 * must treat a provider with NO row at all as enabled, never as an error
 * or as "unknown".
 */
export interface ToolPolicyEntry {
  providerId: string;
  enabled?: boolean | null;
  disabledTools: string[];
}

/** The `tool_policy_get` reply: the STORED rows only, sorted by provider id. */
export interface ToolPolicyReply {
  policies: ToolPolicyEntry[];
}

/**
 * One agent profile, as Settings → Agents saves it. Mirrors `AgentProfile` in
 * the protocol crate (`rename_all = "camelCase"`, unknown fields refused).
 *
 * `id` is the profile's identity and the only key anything uses: the daemon
 * mints one when it is empty, renaming a profile leaves `id` alone, and a child
 * session records the `id` it was started from — so two profiles may share a
 * `name`. `model`, `modeId` and `thinkingOptionId` are the provider's own
 * vocabulary, stored verbatim. `toolOverlay` can only ever remove tools.
 * `spawnPrompt` is daemon-injected text sent at the start of every agent
 * created from this profile; it is absent when the profile carries none.
 *
 * Five fields are **omitted when empty** by the daemon's serde (and older
 * builds wrote profiles without them at all): `icon`, `spawnPrompt`,
 * `thinkingOptionId`, `features`, `toolOverlay`. Absent means the empty value
 * for each of them — a reader must default it, never assume the key exists
 * (`features` blanked the app once, live).
 */
export interface AgentProfile {
  id: string;
  name: string;
  icon?: string | null;
  note: string;
  spawnPrompt?: string;
  provider: string;
  model: string;
  modeId: string;
  thinkingOptionId?: string | null;
  /** Skipped when empty: absent is `{}`, the profile carries no features. */
  features?: Record<string, unknown>;
  toolOverlay?: string[];
  enabledForAgents: boolean;
}

/**
 * The whole stored document: the **ordered** profile list, in the human's
 * order, plus the standing instructions. One document, so a creation reads both
 * halves at one moment and one write cannot leave them disagreeing. An empty
 * document means no profiles AND no standing instructions — never "the last
 * good ones": that is what a corrupt file leaves behind.
 */
export interface AgentProfilesDocument {
  profiles: AgentProfile[];
  standingInstructions: string;
}

/** The `AgentProfilesGet` reply: the stored document, order preserved. */
export interface AgentProfilesReply {
  document: AgentProfilesDocument;
}

/**
 * The three-valued answer to "what does this provider offer". The three are
 * distinct wire values on purpose and must never collapse: `present` — a
 * source answered with a list; `none` — the source can answer and answered
 * "I have none"; `absent` — no source could answer (the agent declared no
 * model shape, the probe failed, the provider is not installed). "The
 * provider published nothing" and "nobody could ask" are different facts.
 */
export type VocabularyState = "present" | "none" | "absent";

/**
 * Who authored a `present` vocabulary list: the provider's own answer on its
 * wire, or the daemon's own mapping (Claude's, Codex's and pi's modes are the
 * launcher's vocabulary — the provider cannot report them). Set only when
 * `state` is `"present"`.
 */
export type VocabularyOrigin = "provider" | "daemon";

/** The models axis of a `ProviderVocabulary` reply. Items are the live manifest's shape, reused. */
export interface VocabularyModels {
  state: VocabularyState;
  origin?: VocabularyOrigin | null;
  /** Empty unless `state` is `"present"`: a `present` with no items is a collapsed absence, never sent. */
  items: SessionModel[];
}

/** The modes axis of a `ProviderVocabulary` reply. Same shape discipline as `VocabularyModels`. */
export interface VocabularyModes {
  state: VocabularyState;
  origin?: VocabularyOrigin | null;
  /** Empty unless `state` is `"present"`. */
  items: SessionModeView[];
}

/**
 * The `provider_vocabulary_get` reply: what one provider offers, so Settings →
 * Agents can author a profile without inventing vocabulary. Mirrors
 * `DaemonMessage::ProviderVocabulary` minus its request id. Wire shape and the
 * three-state behaviour are specified by
 * `reports/remote-agents/SPEC-provider-vocabulary-query.md` §4-§6.
 */
export interface ProviderVocabulary {
  /** The canonical provider id the reply answers for. */
  provider: string;
  models: VocabularyModels;
  modes: VocabularyModes;
  /** How THIS reply was produced: a cached read (`"cache"`) or a fresh probe (`"probe"`). */
  source: "cache" | "probe";
  /** When the cache entry was filled; null for probe replies, which are fresh by definition. */
  probedAtMs?: number | null;
}

/** Result of `provider_update`: the daemon ran `npm install -g <package>@latest` to completion. */
export interface ProviderUpdateOutcome {
  ok: boolean;
  /** npm's exit code; null when npm never ran. */
  exitCode?: number | null;
  /** Already tail-bounded at 64KB by the daemon; the UI shows the last ~500 chars. */
  log: string;
}

export interface ProviderCatalog {
  providers: ProviderInfo[];
  unreadableDirs: number;
}

export type FileTab = "indexed" | "pending" | "stale";

export interface IndexedFile {
  path: string;
  chunks: number;
  updated_at: string;
}

/** A ranked pointer returned by Oracle. It is evidence to read, not prose to consume. */
export type OracleMatchType = "lexical" | "dense" | "dense+lexical" | "dense+reranked";

export interface OracleResult {
  path: string;
  line_start: number;
  line_end: number;
  /**
   * The narrower span inside the range that Oracle's cross-encoder scored as
   * the answer. It is where to look first, not the whole of what is relevant:
   * `snippet` still carries the full chunk and the range is unchanged, so a
   * reader who disagrees with the narrowing loses nothing by ignoring it.
   */
  focus_line_start?: number | null;
  focus_line_end?: number | null;
  /** Redacted by Oracle before IPC; the frontend must never render unredacted source text. */
  snippet: string;
  score: number;
  symbol_name?: string | null;
  match_type?: OracleMatchType | null;
}

export interface OracleSearchResponse {
  query: string;
  results: OracleResult[];
}

export type OracleIndexState = "idle" | "indexing" | "ready" | "incomplete" | "stale" | "error";

export interface OracleResourceBudget {
  max_cpu_percent: number;
  max_memory_mb: number;
  max_parallelism: number;
}

export interface OracleIndexStatus {
  state: OracleIndexState;
  indexed_files: number;
  total_files: number;
  indexed_chunks: number;
  pending_files: number;
  stale_files: number;
  resource_budget: OracleResourceBudget;
  model: OracleModelStatus;
  /** Optional query-time cross-encoder; dense retrieval works without it. */
  reranker: OracleModelStatus | null;
  /** Why the current index is incomplete or waiting on a resource. */
  pause_reason?: string | null;
}

export type OracleModelState =
  | "not_applicable"
  | "missing"
  | "downloading"
  | "ready"
  | "failed"
  | "cancelled";

export interface OracleModelStatus {
  state: OracleModelState;
  model_id: string;
  directory: string;
  file: string | null;
  file_index: number;
  total_files: number;
  bytes_done: number;
  bytes_total: number | null;
  approximate_bytes: number;
  message: string | null;
}

export interface OracleWorkspace {
  path: string | null;
  source: "environment" | "saved" | "unset";
  exists: boolean;
  editable: boolean;
}

export type OracleProgressState =
  | "idle"
  | "running"
  | "completed"
  | "failed"
  | "cancelled"
  | "paused_low_memory"
  | "paused_gpu_temperature"
  | "paused_batch_limit";

export interface OracleIndexProgress {
  state: OracleProgressState;
  completed_files: number;
  total_files: number;
  completed_chunks: number;
  total_chunks: number;
  percentage: number;
  eta_seconds: number | null;
  current_path: string | null;
}

export type OracleHealthCheckState = "ok" | "failed" | "unknown";

export interface OracleHealthCheck {
  id: string;
  state: OracleHealthCheckState;
  message?: string | null;
}

export type OracleHealthState = "healthy" | "degraded" | "unavailable";

export interface OracleHealth {
  state: OracleHealthState;
  checks: OracleHealthCheck[];
  message?: string | null;
}

export interface OracleIndexStats {
  indexed_files: number;
  indexed_chunks: number;
  pending_files: number;
  stale_files: number;
  backend: string;
}

/**
 * Where an arbitrary folder's Oracle index stands.
 *
 * `unreadable` is deliberately distinct from `never_indexed`: a folder whose
 * index cannot be read is not an empty index, and reporting "nothing indexed"
 * there would invite a full re-index over a store that may be intact.
 */
export type OracleFolderIndexState = "never_indexed" | "partial" | "ready" | "unreadable";

/** The answer to "does this folder have an index, and how complete is it?". */
export interface OracleFolderIndexStatus {
  /** The folder that was probed, canonicalized when the filesystem allowed. */
  path: string;
  /** The Oracle data directory derived for that folder. */
  data_dir: string;
  state: OracleFolderIndexState;
  indexed_files: number;
  /** Expected indexable files; zero when there is no index and no walk was done. */
  total_files: number;
  pending_files: number;
  stale_files: number;
  indexed_chunks: number;
  /** Why the folder is not fully indexed; `null` only when the state is `ready`. */
  message: string | null;
}

/**
 * One installed plugin, as discovery found it. A refused plugin is reported
 * here with its reason rather than left out: telling someone who installed a
 * plugin that nothing is installed sends them to fix the wrong thing.
 */
export interface PluginEntry {
  id: string;
  name: string | null;
  version: string | null;
  capabilities: string[];
  /** Rust must serialize the manifest's HTML `ui_entry` into this field. */
  uiEntry: string | null;
  ready: boolean;
  reason: string | null;
  /**
   * Granted invoke budget in serialized bytes. Null when the plugin was
   * refused: there is no verified manifest, and a default here would look
   * like a grant.
   */
  maxPayloadBytes: number | null;
  /**
   * True when the host ceiling cut what the manifest asked for. Null when
   * refused: a clamp is a fact about a declaration we do not have.
   */
  payloadBudgetClamped: boolean | null;
}

export interface PluginInventory {
  root: string;
  plugins: PluginEntry[];
  /** Set when the plugins directory exists but could not be read. */
  problem: string | null;
}

/**
 * Device identity and pairing, mirroring the `devices` / `pairing_*` frames of
 * the daemon protocol (slice 1a). Nested payload structs follow the protocol
 * crate's `rename_all = "camelCase"` convention (`Project`, `Workspace`,
 * `Session`); enum *values* stay snake_case (`answer_permissions`), the same
 * split `PermissionOutcome` already uses.
 */

/** What a paired device is allowed to be. Decided at pairing, never changed later. */
export type PeerRole = "client" | "daemon";

/**
 * One grant a paired device may hold, and one switch in the Devices panel for
 * every role. `view` is the one name `validate_caps` refuses to strip from a
 * `client` peer (the panel holds that switch on); a `daemon` peer may be left
 * with any single one. Every new pairing starts holding all of them
 * (`PEER_DEFAULT_CAPS`, the 2026-09-21 parity decision), so the switches are
 * how a person narrows a device and how a grant is put back. Mirrors
 * `PEER_CAPS` in `crates/devboule-protocol/src/messages.rs` (the DevicesPanel
 * walker test reads that literal so the two cannot drift).
 */
export type Cap =
  | "view"
  | "send"
  | "answer_permissions"
  | "create_sessions"
  | "roster"
  | "search"
  | "admin";

/** Where the daemon keeps its Noise static key. Reported in `Status`. */
export type SecretStore = "keyring" | "file";

/**
 * Whether this device can be reached remotely, as the daemon sees it. `reason`
 * is the daemon's own sentence for a `disabled` or `key_missing` state; the
 * panel renders it verbatim and never invents one.
 */
export interface RemoteState {
  state: "enabled" | "disabled" | "key_missing";
  reason: string | null;
}

/** This device's own identity, from `DevicesReply.selfInfo`. */
export interface SelfInfo {
  deviceId: string;
  displayName: string;
  /** Base64 Noise static public key. */
  publicKey: string;
  /** Hex SHA-256 prefix of `publicKey`; what a person reads aloud at pairing. */
  keyFingerprint: string;
  /** Tailnet addresses this daemon listens on; empty when remote is off. */
  addresses: string[];
  port: number;
  daemonVersion: string;
  protocolVersion: number;
  remote: RemoteState;
}

/** One paired device, as the `peers` table holds it. */
export interface PeerRow {
  deviceId: string;
  displayName: string;
  role: PeerRole;
  publicKey: string;
  keyFingerprint: string;
  /** The only binding that exists today; `relay` is designed but unbuilt. */
  bindingKind: "tailnet";
  bindingNodeName: string | null;
  bindingLoginName: string | null;
  /** `ip:port` learned at pairing. */
  address: string;
  /** Unix milliseconds. */
  pairedAt: number;
  /** Unix milliseconds; `null` while the peer is paired. */
  revokedAt: number | null;
  caps: Cap[];
  /**
   * The local user the pairing was confirmed by, as the daemon recorded it.
   * `null` on a platform without a SID. Daemon-side bookkeeping: the panel
   * shows peers, not this field.
   */
  pairedByUser: string | null;
  online: boolean;
}

/** A pairing the far side asked for and a person here still has to confirm. */
export interface PendingPairing {
  deviceId: string;
  displayName: string;
  role: PeerRole;
  keyFingerprint: string;
  address: string;
  /** Unix milliseconds when the parked pairing gives up. */
  expiresAt: number;
}

/**
 * The `pairing_start` reply: what this device displays. The code is shown once
 * and never logged; it expires on its own, there is no cancel message.
 */
export interface PairingCode {
  code: string;
  /** Unix milliseconds. */
  expiresAt: number;
  /** `ip:port` the other device types. */
  address: string;
}

/** The `devices_list` reply: this device plus everything paired or pending. */
export interface DevicesReply {
  selfInfo: SelfInfo;
  /** Paired rows, revoked ones included: the panel splits them itself. */
  peers: PeerRow[];
  pending: PendingPairing[];
}

/** Handshake readout after the host has spawned a plugin backend. */
export interface PluginBackendStatus {
  pid: number;
  instanceId: string;
  protocolVersion: number;
  capabilities: string[];
  pingOk: boolean;
  /** Host-side ownership token for generation-safe teardown. */
  generation: number;
}

/**
 * The state a message to another session reports back to its sender, in the
 * design's own vocabulary (D6). `accepted` and `queued` are the receiving
 * daemon's intake states; the rest follow that session's turn. A target that
 * is not live is `rejected_absent`. A caller the daemon refused is
 * `rejected_unpaired` when the refusal is about *identity* — a device that is
 * not paired to this session's user — and `rejected_denied` when the caller
 * was authenticated and the message itself was refused (a paired device whose
 * steer may not become an interrupt, for instance, A2-07). A message refused
 * for a brake or for crossing two peers is a wire error, not a receipt.
 */
export type AgentMessageState =
  | "accepted"
  | "queued"
  | "delivered"
  | "started"
  | "completed"
  | "rejected_absent"
  | "rejected_unpaired"
  | "rejected_denied"
  | "expired"
  | "failed";

/**
 * Wire mirror of `ClientMessage::AgentMessageSend`: one session hands text to
 * another session's turn. `fromSession` is imposed by the daemon in the
 * caller's namespace: a local pipe caller names a local session, while a
 * remote peer's validated far id is rendered with the authenticated device
 * as `peer:<device>/<id>`. The device is taken from the connection, never
 * from anything the sender sets. `id` is the request id the receipt echoes.
 *
 * This is the daemon protocol's shape, not a Tauri command payload; the app
 * has no send path for inter-agent messages yet, and the layer that adds one
 * strips the request id from what the frontend sees, as `DevicesReply` does.
 */
export interface AgentMessageSend {
  id: number;
  fromSession: string;
  toSession: string;
  text: string;
  idempotencyKey?: string;
}

/**
 * Wire mirror of `DaemonMessage::AgentMessageReceipt`: the daemon's answer to
 * one `AgentMessageSend`, returned on the sending connection. `id` is the
 * request id being answered, so the sender can pair it with its own send even
 * with several in flight; `state` is where that message ended up.
 */
export interface AgentMessageReceipt {
  id: number;
  state: AgentMessageState;
}
