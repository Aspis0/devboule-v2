// Automatic mirror of delegated design work into the Design side panel.
//
// Which child the panel mirrors: the most recent finished child whose replay
// yields a design artifact. When two children finish close together the later
// arrival wins and the earlier arrival's slow replay is dropped as stale, but
// a human who deliberately opened an older history entry keeps it: the pin
// set by `noteHumanOpenedHistory` blocks every later arrival until the human
// starts a generation or clears it, because a panel that yanks itself out
// from under the reader is worse than one that waits.
//
// Displaying is reading. The mirror replays the child through
// `openDesignHistoryEntry`, whose invoke wrapper only allows `session_attach`
// and `session_detach` and rejects resume, send, spawn and close before the
// bridge can receive them. The mirror itself never calls the daemon directly:
// it writes the extracted artifact into the app store, which costs nothing
// and allocates nothing. No attach here spawns, resumes or sends.
//
// Only an artifact is ever written. A replay that yields nothing writes
// nothing: no card, no error, no warning. The common case is a child whose
// job was never design (a coder finishing leaves no fenced HTML behind),
// and that is not an error to display in a panel about design work.
//
// The write leaves a complete session behind — host, document, messages —
// because the surface's load effect stands down on a non-null document and
// replaces the transcript otherwise. Establishing the session here (one small
// file, next to the write it protects) is harder to break than teaching the
// shared load path to merge: every future loading change would have to
// re-derive the merge, while this state is byte-identical to one the app
// already produces (a fresh load followed by an appended card).
//
// Every value import below is type-only on purpose. This module is scheduled
// from the shared session event pipeline, so a static runtime edge into the
// store, the history reopen or the agent host would load those graphs into
// every session test's partial mocks; the async runner resolves them after
// the handler returns, which is also what keeps the event callback unblocked.
import type { useAppStore as useAppStoreType } from "../../store/appStore";
import type { SessionEvent } from "../../types/ipc";
import type { DesignHost } from "./designHost";
import type { openDesignHistoryEntry as openHistoryType } from "./designHistoryOpen";

/** The `child_finished` arm of the session event union. */
type ChildFinishedEvent = Extract<SessionEvent, { type: "child_finished" }>;

export interface DelegatedMirrorDeps {
  /** Test seam: defaults to the read-only history reopen. */
  openHistory?: typeof openHistoryType;
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
  // Dynamic, not static: `agentHost` imports `AgentSession`, which imports
  // this module for the scheduling call, so a static import would close a
  // module cycle through the shared event pipeline. The construction itself
  // performs no daemon I/O; the host owns no session until a generation.
  const { createAgentHost } = await import("./agentHost");
  return createAgentHost();
}

/**
 * The replay's HTML, or null when there is no design work to show. Both
 * non-artifact outcomes resolve null, silently: a timeout proves nothing
 * about the transcript, and a failed extraction means it holds no fenced
 * HTML — which covers the oversized artifact too (unrenderable in the
 * panel; reachable through its history entry, where the surface reports the
 * too-large message itself). The panel keeps whatever it showed on either.
 */
function replayChildArtifact(
  childSessionId: string,
  open: typeof openHistoryType,
): Promise<string | null> {
  return new Promise((resolve) => {
    open(childSessionId, {
      onResult: (result) => {
        if (result.status === "loading") return;
        resolve(result.status === "artifact" ? result.html : null);
      },
    });
  });
}

async function writeDelegatedArtifact(
  childSessionId: string,
  displayName: string,
  html: string,
  ensureHost: () => Promise<DesignHost>,
  store: typeof useAppStoreType,
): Promise<void> {
  // The human pinned an entry while the replay was in flight, or a duplicate
  // arrival's replay finished second: neither may write.
  if (humanPinnedChildId !== null) return;
  if (mirroredChildId === childSessionId) return;
  let host = store.getState().designSession.host;
  if (host === null) {
    // Design was never opened this run, so the store has no host to write
    // through. Creating one is what lets the panel mirror the delegation
    // instead of keeping its never-opened empty state.
    host = await ensureHost();
    store.getState().setDesignHost(host);
  }
  const title = displayName.length > 0 ? displayName : childSessionId.slice(0, 8);
  const card = {
    id: `${DELEGATED_DESIGN_MESSAGE_PREFIX}${childSessionId}`,
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
  if (current.host !== host) return;
  if (current.document !== null) {
    store.getState().setDesignMessages(host, (messages) => [...messages, { ...card }]);
    return;
  }
  // No document: establish the host's own loaded document together with the
  // card. A fresh host's loader is pure defaults, so nothing here is
  // invented, and the re-reads bracket every await so this write never
  // clobbers a document that appeared while it waited. What they cannot
  // cover: the surface's own load calls setDesignDocument with the loaded
  // messages unconditionally, so one already in flight when this lands still
  // replaces the card. Both loaders resolve in microtasks, so the window is
  // microtask-wide, and it cannot be closed from this side.
  const document = await host.loadDocument();
  const settled = store.getState().designSession;
  if (settled.host !== host) return;
  if (settled.document !== null) {
    store.getState().setDesignMessages(host, (messages) => [...messages, { ...card }]);
    return;
  }
  store.getState().setDesignDocument(host, document, [...settled.messages, { ...card }]);
}

/**
 * Schedules the mirror for one finished child. Fire-and-forget from the
 * shared session event pipeline (`AgentSession.handleEvent`): the synchronous
 * prefix is the sequence bump only — it performs no daemon I/O, reads no
 * store and takes no dependency, so nothing here blocks the event callback
 * and nothing can deadlock against a cleared session. Every check runs in
 * the async runner after the store resolves: a cleared session first drops
 * the pin and the mirror record (both died with the card), then the pin and
 * the duplicate checks run before any replay starts, so a pinned arrival
 * performs no daemon read at all. Failures warn and change nothing; a lost
 * mirror must not take the pipeline down with it.
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
      const open =
        deps?.openHistory ?? (await import("./designHistoryOpen")).openDesignHistoryEntry;
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
      const html = await replayChildArtifact(childSessionId, open);
      if (html === null) return;
      // Only a newer *write* drops this one: an arrival that produced
      // nothing consumes no slot, so a slow design replay still lands after
      // a fast empty one. Among arrivals with content the last writer wins,
      // which converges on the most recent one either way.
      if (sequence < writtenSequence) return;
      await writeDelegatedArtifact(childSessionId, displayName, html, ensureHost, store);
      writtenSequence = sequence;
      mirroredChildId = childSessionId;
    } catch (error) {
      console.warn(`Could not mirror the finish of child session ${childSessionId}.`, error);
    }
  })();
}
