// Automatic mirror of delegated design work into the Design side panel.
//
// Which child the panel mirrors: the most recent finished child whose
// deposited artifact yields a design. When two children finish close
// together the later arrival wins and the earlier arrival's slow read is
// dropped as stale, but a human who deliberately opened an older history
// entry keeps it: the pin set by `noteHumanOpenedHistory` blocks every
// later arrival until the human starts a generation or clears it, because
// a panel that yanks itself out from under the reader is worse than one
// that waits.
//
// Displaying is reading. The mirror reads the finished child's deposited
// bytes through `sessionAttachmentRead` and extracts fenced HTML from the
// decoded markdown: no replay, no attach, and it writes the extracted
// artifact into the app store. No read here spawns, resumes or sends.
//
// Only an artifact is ever written. A finish that yields nothing writes
// nothing: no card, no error, no warning. The common case is a child whose
// job was never design (a coder finishing leaves no fenced HTML behind),
// and that is not an error to display in a panel about design work.
//
// The write leaves a complete session behind — host, document, messages —
// because the surface's load effect stands down on a non-null document and
// replaces the transcript otherwise. Establishing it here, next to the write
// it protects, is harder to break than teaching the shared load path to
// merge: every future loading change would have to re-derive the merge.
//
// Every value import below is type-only on purpose. This module is scheduled
// from the shared session event pipeline, so a static runtime edge into the
// store, the history reopen or the agent host would load those graphs into
// every session test's partial mocks; the async runner resolves them after
// the handler returns, which is also what keeps the event callback unblocked.
import type { useAppStore as useAppStoreType } from "../../store/appStore";
import type { SessionEvent } from "../../types/ipc";
import type { AttachmentReference, StoredAttachment } from "../../lib/tauri";
import type { DesignHost } from "./designHost";
import { parseAttachmentReference } from "./attachmentReference";

/** The `child_finished` arm of the session event union. */
type ChildFinishedEvent = Extract<SessionEvent, { type: "child_finished" }>;

export interface DelegatedMirrorDeps {
  /** Test seam: defaults to the real attachment read. */
  readStored?: (reference: AttachmentReference) => Promise<StoredAttachment>;
  /** Test seam: defaults to creating the real Design host. */
  ensureHost?: () => Promise<DesignHost>;
  /** Test seam: defaults to the real app store. */
  store?: typeof useAppStoreType;
}

/**
 * The id prefix of every card this mirror writes. Exported so the surface's
 * generation count can recognise these cards as readings, not generations —
 * one spelling, in the module that mints it.
 */
export const DELEGATED_DESIGN_MESSAGE_PREFIX = "delegated-design-";

let mirrorSequence = 0;
let mirroredChildId: string | null = null;
let writtenSequence = 0;
let humanPinnedChildId: string | null = null;

/**
 * Records that the human deliberately opened a history entry. While pinned,
 * later delegated arrivals neither replay nor write, including the pinned
 * child itself (it is already on screen, so a second card would duplicate
 * it). Wired from `DesignSurface.openHistoryEntry`; nothing else may set it.
 */
export function noteHumanOpenedHistory(sessionId: string): void {
  humanPinnedChildId = sessionId;
}

/**
 * Releases the human's pin when they move on: a new generation is their own
 * work, so later delegations may mirror again. Wired from
 * `DesignSurface.startGeneration`; the pin is never cleared by an arrival.
 */
export function clearDelegatedMirrorPin(): void {
  humanPinnedChildId = null;
}

/** Resets all mirror state. Tests only; production state lives for the run. */
export function resetDelegatedMirrorForTests(): void {
  mirrorSequence = 0;
  mirroredChildId = null;
  writtenSequence = 0;
  humanPinnedChildId = null;
}

async function defaultEnsureHost(): Promise<DesignHost> {
  // Dynamic: a static import would close a module cycle through the shared
  // event pipeline. Construction performs no daemon I/O.
  const { createAgentHost } = await import("./agentHost");
  return createAgentHost();
}

/**
 * The stored markdown of the first markdown part, or null when there is
 * nothing this panel can show — no markdown part, an unparseable url, or
 * markdown with no fenced HTML. Silent either way; the panel keeps what it
 * showed. A failed read never answers here: the rejection propagates to the
 * runner's catch, which warns and writes nothing.
 */
async function readChildArtifact(
  artifacts: ChildFinishedEvent["artifacts"],
  readStored: (reference: AttachmentReference) => Promise<StoredAttachment>,
  extractFencedHtml: (text: string) => string | undefined,
): Promise<string | null> {
  for (const artifact of artifacts) {
    for (const part of artifact.parts) {
      // A part of another type is not markdown and must not be extracted
      // from; the first markdown part decides, even when it refuses.
      if (part.mimeType !== "text/markdown") continue;
      const reference = parseAttachmentReference(part);
      if (reference === null) return null;
      const stored = await readStored(reference);
      return extractFencedHtml(decodeAttachmentText(stored.data)) ?? null;
    }
  }
  return null;
}

/**
 * Base64 to text, through bytes: `atob` yields one byte per character, so
 * reading its output as text corrupts every non-ASCII byte into mojibake.
 */
function decodeAttachmentText(data: string): string {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return new TextDecoder().decode(bytes);
}

/** Whether the transcript already holds this mirror's card: same id, same React key. */
function hasDelegatedCard(messages: readonly { id: string }[], cardId: string): boolean {
  return messages.some((message) => message.id === cardId);
}

/** Writes one card. A pin landed mid-read, a host changed under the
 * write, or a card already there all answer false, so only shown work
 * marks the slot. */
async function writeDelegatedArtifact(
  childSessionId: string,
  displayName: string,
  html: string,
  ensureHost: () => Promise<DesignHost>,
  store: typeof useAppStoreType,
): Promise<boolean> {
  // The human pinned an entry while the read was in flight, or a duplicate
  // arrival's read finished second: neither may write.
  if (humanPinnedChildId !== null) return false;
  if (mirroredChildId === childSessionId) return false;
  let host = store.getState().designSession.host;
  if (host === null) {
    const fresh = await ensureHost();
    const appeared = store.getState().designSession.host;
    if (appeared !== null) {
      // A session appeared while the factory resolved: adopt it and drop
      // the fresh host, which owns no daemon resources. Establishing it
      // would zero the live session and strand the mounted surface.
      host = appeared;
    } else {
      // Design was never opened this run, so the store has no host to write
      // through. Creating one is what lets the panel mirror the delegation
      // instead of keeping its never-opened empty state.
      store.getState().setDesignHost(fresh);
      host = fresh;
    }
  }
  const title = displayName.length > 0 ? displayName : childSessionId.slice(0, 8);
  const cardId = `${DELEGATED_DESIGN_MESSAGE_PREFIX}${childSessionId}`;
  const card = {
    id: cardId,
    role: "assistant",
    status: "done",
    title,
    desc: "Finished by a delegated agent.",
    sources: [],
    nodeIds: [],
    // A history entry records neither the shape the run asked for nor how
    // many blocks its reply carried, so a mirrored artifact carries no
    // outputMode and no fenced count — the same absence as a reopened one.
    artifactHtml: html,
  } as const;
  const current = store.getState().designSession;
  if (current.host !== host) return false;
  if (current.document !== null) {
    if (hasDelegatedCard(current.messages, cardId)) return false;
    store.getState().setDesignMessages(host, (messages) => [...messages, { ...card }]);
    return true;
  }
  // No document: establish the host's own loaded document with the card —
  // pure defaults from a fresh loader, nothing invented. The re-read below
  // buys the append branch when the surface's load lands first; it does NOT
  // stop the surface replacing this card afterwards, because
  // `DesignSurface`'s own load calls `setDesignDocument` with the loaded
  // messages unconditionally. What rules that ordering out today is microtask
  // FIFO over a synchronous default factory — give `loadDocument` real I/O
  // and the replacement becomes reachable while this code still looks safe.
  const document = await host.loadDocument();
  const settled = store.getState().designSession;
  if (settled.host !== host) return false;
  if (settled.document !== null) {
    if (hasDelegatedCard(settled.messages, cardId)) return false;
    store.getState().setDesignMessages(host, (messages) => [...messages, { ...card }]);
    return true;
  }
  // No await sits between each check and its write, so concurrent
  // deliveries cannot both pass: the second sees the first's card.
  if (hasDelegatedCard(settled.messages, cardId)) return false;
  store.getState().setDesignDocument(host, document, [...settled.messages, { ...card }]);
  return true;
}

/**
 * Schedules the mirror for one finished child, fire-and-forget from
 * `AgentSession.handleEvent`. The synchronous prefix is the `completed`
 * gate and the sequence bump only — no store reads, no daemon I/O. Every
 * other check runs after the store resolves; a cleared session first drops
 * the pin and the mirror record, and a pinned arrival performs no daemon
 * read at all. Failures warn and change nothing.
 */
export function scheduleDelegatedDesignMirror(
  event: ChildFinishedEvent,
  deps?: DelegatedMirrorDeps,
): void {
  const childSessionId = event.childSessionId;
  const displayName = typeof event.displayName === "string" ? event.displayName.trim() : "";
  // Only a child that finished its work is mirrored. The daemon reports
  // `completed` exactly when the agent stopped normally or exited zero —
  // every other stop reason fails closed (`child_finish_state`,
  // `stop_reason_state`) — so any other state leaves its transcript to the
  // history entry, where the human reads the true state.
  if (event.state !== "completed") return;
  const sequence = ++mirrorSequence;

  void (async () => {
    try {
      const store = deps?.store ?? (await import("../../store/appStore")).useAppStore;
      const ensureHost = deps?.ensureHost ?? defaultEnsureHost;
      // A cleared session drops the pin and the mirror record with it: the
      // entry the human was reading and the card the mirror wrote are both
      // gone, so stale state must neither block the next delegation nor skip
      // re-mirroring the same child after its card was cleared.
      if (store.getState().designSession.host === null) {
        humanPinnedChildId = null;
        mirroredChildId = null;
        writtenSequence = 0;
      }
      if (humanPinnedChildId !== null) return;
      if (mirroredChildId === childSessionId) return;
      const readStored =
        deps?.readStored ?? (await import("../../lib/tauri")).sessionAttachmentRead;
      const { extractFencedHtml } = await import("./agentHost");
      const html = await readChildArtifact(event.artifacts, readStored, extractFencedHtml);
      if (html === null) return;
      // Only a newer *write* drops this one, and only a write marks the
      // slot: an arrival that produced nothing — or whose write found
      // nothing to do — consumes nothing, so a slow design read still
      // lands after a fast empty one. Among arrivals with content the last
      // writer wins, which converges on the most recent one either way.
      if (sequence < writtenSequence) return;
      if (await writeDelegatedArtifact(childSessionId, displayName, html, ensureHost, store)) {
        writtenSequence = sequence;
        mirroredChildId = childSessionId;
      }
    } catch (error) {
      console.warn(`Could not mirror the finish of child session ${childSessionId}.`, error);
    }
  })();
}
