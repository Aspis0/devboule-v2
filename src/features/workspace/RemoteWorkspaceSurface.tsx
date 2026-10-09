import { useCallback, useEffect, useRef, useState } from "react";
import {
  allocRemoteSubscriptionId,
  createRemoteEventChannel,
  remoteHostClaim,
  remoteHostClose,
  remoteHostCreate,
  remoteHostInterrupt,
  remoteHostList,
  remoteHostPermissionRespond,
  remoteHostProviders,
  remoteHostResize,
  remoteHostSend,
  remoteHostStop,
  remoteSessionAttach,
  remoteSessionDetach,
  type RemoteEventChannel,
} from "../../lib/tauri";
import { errorSentence } from "../../lib/errorSentence";
import { useMenuOpen } from "../../lib/menuOpen";
import { PermissionCard, type PermissionAnswer } from "../../components/PermissionCard";
import { WorkspaceComposer } from "./WorkspaceComposer";
import { WorkspaceNewTabMenu } from "./strip/WorkspaceNewTabMenu";
import { createTerminalView, type TerminalViewHandle } from "../terminal/createTerminalView";
import {
  isAgentKind,
  type PermissionRequest,
  type ProviderInfo,
  type Session,
  type SessionEvent,
  type SessionKind,
} from "../../types/ipc";
import { sessionCreateFromProvider, workspacePickerProviders } from "./workspaceSessions";

interface RemoteWorkspaceSurfaceProps {
  deviceId: string;
  workspaceId: string;
  hostOnline: boolean;
}

/** One event as one line of the read-only transcript. Events that are not
 * words (state, task snapshots) carry no line; the tab's own status comes from
 * the host's session rows. */
function lineOf(event: SessionEvent): string | null {
  switch (event.type) {
    case "output":
      return event.data;
    case "agent_message":
    case "agent_user_message":
    case "session_notice":
      return event.text;
    default:
      return null;
  }
}

/** How many transcript lines one remote tab keeps; older ones are dropped
 * from the DOM and memory, exactly like the local transcript's cap. */
const MAX_REMOTE_LINES = 2000;

/** The bulk transcript events the relay may drop and replay; everything else
 * is a status the tabs must reflect immediately. */
function isBulkTranscript(event: SessionEvent): boolean {
  return (
    event.type === "output" ||
    event.type === "agent_message" ||
    event.type === "agent_user_message" ||
    event.type === "agent_stderr" ||
    event.type === "agent_thought"
  );
}

/** The one word a tab's live status prints, from the host's own row. */
function statusWord(session: Session): string | null {
  if (session.attention !== undefined) return "waiting";
  if (session.state.type === "ended") return "ended";
  if (session.activity === "working") return "working";
  return null;
}

interface RemoteCard {
  sessionId: string;
  subscriptionId: number;
  request: PermissionRequest;
}

interface CreateFailed {
  kind: "agent" | "terminal";
  provider: ProviderInfo | undefined;
  /** The retry identity, minted once per user intent and kept across the
   * retry affordance: an explicit retry reuses it, and nothing resends on
   * its own, because after a transport failure the outcome is unknown. */
  key: string;
  message: string;
}

/**
 * The remote workspace's surface: the host's sessions for this workspace as
 * tabs, the opened one driven live. Every frame comes from the owning daemon
 * and every write goes back to it — the local daemon never runs the remote
 * provider, shell or filesystem operation — and while the host is gone the
 * tab row says one short word and the stream resumes on the next online edge
 * with a fresh subscription.
 *
 * Creation reuses the local new-tab menu and the host's own provider
 * catalog; agent input reuses the local composer; permission cards reuse the
 * local card with the human's answer travelling as the human's (no
 * confirmation of its own); terminals reuse the terminal view. The human
 * controls the host with no confirmation card, exactly like the local
 * surface: confirmation cards stay reserved for agent-originated
 * cross-machine commands, which travel the card path, not this one.
 */
export function RemoteWorkspaceSurface({
  deviceId,
  workspaceId,
  hostOnline,
}: RemoteWorkspaceSurfaceProps) {
  const [sessions, setSessions] = useState<readonly Session[]>([]);
  const [openSessionId, setOpenSessionId] = useState<string | null>(null);
  const [lines, setLines] = useState<readonly string[]>([]);
  const [streamState, setStreamState] = useState<"idle" | "streaming" | "offline">("idle");
  // The attach that completed, or null while none is up: sends, claims and
  // the terminal view all wait for it, so a write can never race the attach
  // it needs.
  const [attached, setAttached] = useState<{
    sessionId: string;
    subscriptionId: number;
  } | null>(null);
  // Permission cards the host raised on its sessions, answered from here.
  const [cards, setCards] = useState<readonly RemoteCard[]>([]);
  // A gap marker (the relay dropped bulk events) closes the stream and
  // re-attaches with a fresh subscription, which replays the transcript; the
  // counter is what makes the attach effect run again for the same session.
  const [resyncNonce, setResyncNonce] = useState(0);
  const subscriptionRef = useRef(0);
  const openRef = useRef<string | null>(null);
  // One operation at a time: the link serves a single attach/detach, so a
  // tab switch serializes instead of racing `Busy` refusals. The token makes
  // latest-wins: a failure from an operation the user already replaced never
  // paints the current surface offline.
  const chainRef = useRef<Promise<void>>(Promise.resolve());
  const operationRef = useRef(0);
  const lastAttachAtRef = useRef(0);
  // A gap is a data event, not a flapping edge: its reattach is immediate,
  // but a transcript that overflows the relay on every replay must not loop.
  // One resync per backoff window; gaps inside it mean the replay itself is
  // overflowing, so the tail we already have is kept rather than re-gapped.
  const forceReattachRef = useRef(false);
  const lastResyncAtRef = useRef(0);
  const resyncBackoffRef = useRef(1000);
  const reattachTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Line appends are batched per microtask and bounded, so a long replay does
  // not re-render the whole history once per chunk.
  const pendingLinesRef = useRef<string[]>([]);
  const flushScheduledRef = useRef(false);
  // Event-triggered reloads are coalesced to at most one per second, with the
  // last ask delivered as a trailing one.
  const lastLoadAtRef = useRef(0);
  const loadTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // The rows the roster last reported, for the stream callback: a terminal's
  // output goes to its view, an agent's to the transcript.
  const sessionsRef = useRef<readonly Session[]>([]);
  sessionsRef.current = sessions;
  // One xterm writer per open terminal session, registered by its pane.
  const terminalWritersRef = useRef(new Map<string, (data: string) => void>());
  // The new-tab menu, the provider picker, the create in flight and the
  // indeterminate outcome with its explicit retry.
  const addButtonRef = useRef<HTMLButtonElement>(null);
  const [newTabOpen, setNewTabOpen] = useState(false);
  const [providerPick, setProviderPick] = useState<readonly ProviderInfo[] | null>(null);
  const [creating, setCreating] = useState(false);
  const [createFailed, setCreateFailed] = useState<CreateFailed | null>(null);
  // The provider picker is a menu like any other: the shared hook lets a
  // menu-closer dismiss it, and walking away from it ends the choice.
  useMenuOpen(providerPick !== null, () => setProviderPick(null));

  // The host's own session rows for this workspace. The roster is live, not a
  // one-shot snapshot: it reloads on the online edge, on a working session's
  // important events, and on a short interval while the host answers — an
  // agent created, exited or moved into approval elsewhere updates the tabs.
  const load = useCallback(async () => {
    try {
      const body = await remoteHostList(deviceId, { kind: "sessions" });
      if (body.list !== "sessions") return;
      const rows = body.rows.filter((row) => row.workspaceId === workspaceId);
      setSessions(rows);
      // A session the host no longer lists (exited and reaped, or another
      // workspace) cannot stay selected: its transcript would outlive it.
      // Its cards go with it: answering a card the host forgot resolves
      // nothing.
      const listed = new Set(rows.map((row) => row.id));
      setCards((current) => current.filter((card) => listed.has(card.sessionId)));
      setOpenSessionId((current) =>
        current !== null && !rows.some((row) => row.id === current) ? null : current,
      );
    } catch {
      // Keep the last list; the row's own offline state says why.
    }
  }, [deviceId, workspaceId]);
  useEffect(() => {
    void load();
  }, [load, hostOnline]);
  // A reload asked for by a stream event waits its turn: one per second at
  // most, and the last ask always runs.
  const requestLoad = useCallback(() => {
    const since = Date.now() - lastLoadAtRef.current;
    if (since >= 1000) {
      lastLoadAtRef.current = Date.now();
      void load();
      return;
    }
    if (loadTimerRef.current !== null) clearTimeout(loadTimerRef.current);
    loadTimerRef.current = setTimeout(() => {
      loadTimerRef.current = null;
      lastLoadAtRef.current = Date.now();
      void load();
    }, 1000 - since);
  }, [load]);
  // The slow roster re-read runs only while the window is visible: a hidden
  // window has nothing to repaint, and coming back re-reads once and resumes
  // the interval.
  useEffect(() => {
    if (!hostOnline) return;
    let timer: ReturnType<typeof setInterval> | null = null;
    const start = () => {
      if (timer === null) timer = setInterval(() => void load(), 5000);
    };
    const stop = () => {
      if (timer !== null) {
        clearInterval(timer);
        timer = null;
      }
    };
    const onVisibility = () => {
      if (document.hidden) {
        stop();
        return;
      }
      void load();
      start();
    };
    if (document.hidden) stop();
    else start();
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      stop();
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [hostOnline, load]);
  // A coalesced trailing reload must not outlive the surface.
  useEffect(
    () => () => {
      if (loadTimerRef.current !== null) {
        clearTimeout(loadTimerRef.current);
        loadTimerRef.current = null;
      }
    },
    [],
  );

  // Open the session's stream, reattaching with a fresh subscription on the
  // online edge. Every operation is serialized on one chain and carries a
  // token, so a tab switch detaches before the next attach and a stale
  // failure cannot paint the surface offline.
  useEffect(() => {
    const target = openSessionId;
    const previous = openRef.current;
    openRef.current = target;
    operationRef.current += 1;
    const token = operationRef.current;
    const forced = forceReattachRef.current;
    forceReattachRef.current = false;
    const run = chainRef.current.then(async () => {
      if (token !== operationRef.current) return;
      // The previous subscription goes back before the next one opens: a
      // session switch and a gap's replay both detach on this chain, never
      // fire-and-forget, so the link is never asked for two streams at once.
      const previousSubscription = subscriptionRef.current;
      const detachSession = previous ?? target;
      if (previousSubscription !== 0 && detachSession !== null && (previous !== target || forced)) {
        subscriptionRef.current = 0;
        setAttached(null);
        await remoteSessionDetach(deviceId, detachSession, previousSubscription).catch(
          () => undefined,
        );
      }
      if (token !== operationRef.current) return;
      if (target === null) {
        setStreamState("idle");
        return;
      }
      if (!hostOnline) {
        setStreamState("offline");
        // Coming back is a fresh edge: the debounce window does not survive
        // an offline period.
        lastAttachAtRef.current = 0;
        return;
      }
      // A flapping online edge must not storm attaches: a reattach for the
      // *same* session inside the debounce window is coalesced into one. A
      // user switching tabs, and a gap's replay, are never debounced.
      const sameSession = previous === target;
      const since = Date.now() - lastAttachAtRef.current;
      if (sameSession && !forced && since < 750) {
        if (reattachTimerRef.current !== null) clearTimeout(reattachTimerRef.current);
        reattachTimerRef.current = setTimeout(() => {
          reattachTimerRef.current = null;
          setResyncNonce((value) => value + 1);
        }, 750);
        return;
      }
      lastAttachAtRef.current = Date.now();
      subscriptionRef.current = allocRemoteSubscriptionId();
      const subscriptionId = subscriptionRef.current;
      setLines([]);
      setStreamState("streaming");
      const channel: RemoteEventChannel = createRemoteEventChannel((message) => {
        if (message.deviceId !== deviceId || message.sessionId !== target) return;
        if (message.subscriptionId !== subscriptionId) return;
        if (message.kind === "gap") {
          // The relay lost bulk events. Re-attach so the host replays —
          // but at most once per backoff window: a transcript that
          // overflows the relay on every replay keeps its tail instead of
          // looping through gap and reattach forever.
          const now = Date.now();
          if (now - lastResyncAtRef.current < resyncBackoffRef.current) {
            return;
          }
          lastResyncAtRef.current = now;
          resyncBackoffRef.current = Math.min(resyncBackoffRef.current * 2, 30_000);
          // The detach rides the serialized chain with the attach.
          forceReattachRef.current = true;
          setLines([]);
          setResyncNonce((value) => value + 1);
          return;
        }
        // A healthy stream clears the resync backoff: a later genuine gap
        // replays right away.
        resyncBackoffRef.current = 1000;
        const event = message.envelope.event;
        // A card the host raised or resolved changes what the pane shows;
        // the roster row follows on the next read.
        if (event.type === "permission_request") {
          setCards((current) =>
            current.some(
              (card) => card.sessionId === target && card.request.toolCallId === event.toolCallId,
            )
              ? current
              : [...current, { sessionId: target, subscriptionId, request: event }],
          );
          requestLoad();
          return;
        }
        if (event.type === "permission_resolved" || event.type === "permission_answered") {
          const cardId = event.type === "permission_resolved" ? event.toolCallId : event.cardId;
          setCards((current) =>
            current.filter(
              (card) => !(card.sessionId === target && card.request.toolCallId === cardId),
            ),
          );
          requestLoad();
          return;
        }
        // A terminal's output goes to its view; an agent's to the transcript.
        // States, terminal exits and other non-words still move the roster.
        const kind = sessionsRef.current.find((row) => row.id === target)?.kind;
        if (kind !== undefined && !isAgentKind(kind)) {
          if (event.type === "output") {
            terminalWritersRef.current.get(target)?.(event.data);
          } else if (!isBulkTranscript(event)) {
            requestLoad();
          }
          return;
        }
        const line = lineOf(event);
        if (line !== null) appendLine(line);
        // A state, a terminal exit or a permission card changes what the
        // tabs should say; the roster is re-read rather than left stale,
        // coalesced so a burst of events is one read.
        if (!isBulkTranscript(event)) requestLoad();
      });
      try {
        await remoteSessionAttach(deviceId, target, subscriptionId, channel);
        if (token === operationRef.current && subscriptionRef.current === subscriptionId) {
          setAttached({ sessionId: target, subscriptionId });
        }
      } catch {
        if (token === operationRef.current) {
          setAttached(null);
          setStreamState("offline");
        }
      }
    });
    chainRef.current = run;
    return () => {
      if (reattachTimerRef.current !== null) {
        clearTimeout(reattachTimerRef.current);
        reattachTimerRef.current = null;
      }
    };
    // The roster is read through a ref on purpose: a status change must not
    // re-run the stream it is describing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deviceId, hostOnline, load, openSessionId, requestLoad, resyncNonce]);

  // Give the stream back when the surface leaves.
  useEffect(
    () => () => {
      const open = openRef.current;
      if (open !== null) {
        void remoteSessionDetach(deviceId, open, subscriptionRef.current).catch(() => undefined);
      }
    },
    [deviceId],
  );

  const select = useCallback((sessionId: string) => {
    setOpenSessionId(sessionId);
  }, []);

  function appendLine(line: string): void {
    pendingLinesRef.current.push(line);
    if (flushScheduledRef.current) return;
    flushScheduledRef.current = true;
    queueMicrotask(() => {
      flushScheduledRef.current = false;
      const batch = pendingLinesRef.current;
      pendingLinesRef.current = [];
      setLines((current) => [...current, ...batch].slice(-MAX_REMOTE_LINES));
    });
  }

  // One create, one retry identity: minted when the person asks, kept while
  // the failure stands, reused by the explicit retry and dropped on success
  // or dismissal. Nothing resends on its own.
  const doCreate = useCallback(
    async (kind: "agent" | "terminal", provider: ProviderInfo | undefined, key: string) => {
      setCreating(true);
      setCreateFailed(null);
      try {
        const args =
          kind === "terminal"
            ? { kind: "terminal" as SessionKind, provider: null as string | null }
            : sessionCreateFromProvider(provider);
        const session = await remoteHostCreate({
          deviceId,
          workspaceId,
          kind: args.kind,
          provider: args.provider,
          idempotencyKey: key,
        });
        setCreating(false);
        await load();
        setOpenSessionId(session.id);
      } catch (cause: unknown) {
        setCreating(false);
        setCreateFailed({ kind, provider, key, message: errorSentence(cause).sentence });
      }
    },
    [deviceId, load, workspaceId],
  );

  const startCreate = useCallback(
    (kind: "agent" | "terminal", provider?: ProviderInfo) => {
      setNewTabOpen(false);
      setProviderPick(null);
      void doCreate(kind, provider, crypto.randomUUID());
    },
    [doCreate],
  );

  // The agent entry offers the host's own catalog: one capable provider goes
  // straight through, several open the picker, none stops with one line.
  const handleNewAgent = useCallback(async () => {
    setNewTabOpen(false);
    setCreateFailed(null);
    let catalog: readonly ProviderInfo[];
    try {
      catalog = (await remoteHostProviders(deviceId)).providers;
    } catch (cause: unknown) {
      setCreateFailed({
        kind: "agent",
        provider: undefined,
        key: crypto.randomUUID(),
        message: errorSentence(cause).sentence,
      });
      return;
    }
    const capable = workspacePickerProviders([...catalog]);
    if (capable.length === 1 && capable[0] !== undefined) {
      startCreate("agent", capable[0]);
    } else if (capable.length > 1) {
      setProviderPick(capable);
    } else {
      setProviderPick([]);
    }
  }, [deviceId, startCreate]);

  const handleNewTerminal = useCallback(() => {
    startCreate("terminal");
  }, [startCreate]);

  const retryCreate = useCallback(() => {
    const failed = createFailed;
    if (failed === null) return;
    void doCreate(failed.kind, failed.provider, failed.key);
  }, [createFailed, doCreate]);

  const dismissCreateFailed = useCallback(() => {
    setCreateFailed(null);
  }, []);

  const closeSession = useCallback(
    async (sessionId: string) => {
      try {
        await remoteHostClose(deviceId, sessionId);
      } catch {
        // The roster below says whether it went.
      }
      await load();
    },
    [deviceId, load],
  );

  const archiveSession = useCallback(
    async (sessionId: string, subscriptionId: number) => {
      try {
        await remoteHostStop(deviceId, sessionId, subscriptionId);
      } catch {
        // The roster below says whether it went.
      }
      await load();
    },
    [deviceId, load],
  );

  const openSession = sessions.find((row) => row.id === openSessionId) ?? null;
  const openIsAgent = openSession !== null && isAgentKind(openSession.kind);
  const openCards =
    openSessionId === null ? [] : cards.filter((card) => card.sessionId === openSessionId);
  const turnActive = openSession?.activity === "working";
  const sendReady = attached !== null && attached.sessionId === openSessionId && hostOnline;

  const stopTurn = useCallback(() => {
    if (attached === null || attached.sessionId !== openSessionId) return;
    void remoteHostInterrupt(deviceId, attached.sessionId, attached.subscriptionId).catch(
      () => undefined,
    );
  }, [attached, deviceId, openSessionId]);

  const answerCard = useCallback(
    (card: RemoteCard) => async (response: PermissionAnswer) => {
      await remoteHostPermissionRespond({
        deviceId,
        sessionId: card.sessionId,
        subscriptionId: card.subscriptionId,
        requestId: card.request.toolCallId,
        outcome: response.outcome,
        ...(response.optionId === undefined ? {} : { optionId: response.optionId }),
        ...(response.answer === undefined ? {} : { answer: response.answer }),
        idempotencyKey: crypto.randomUUID(),
      });
    },
    [deviceId],
  );

  return (
    <div className="workspace-remote-surface">
      <div className="workspace-remote-tabs" role="tablist" aria-label="Remote sessions">
        {sessions.map((session) => (
          <span key={session.id} className="workspace-remote-tab">
            <button
              type="button"
              role="tab"
              aria-selected={session.id === openSessionId}
              className="workspace-secondary-action"
              onClick={() => select(session.id)}
            >
              {session.title.trim() === "" ? session.id : session.title}
              {statusWord(session) === null ? null : (
                <span className="workspace-remote-status">{statusWord(session)}</span>
              )}
            </button>
            <button
              type="button"
              className="workspace-remote-close"
              aria-label={`Close ${session.title.trim() === "" ? session.id : session.title}`}
              onClick={() => void closeSession(session.id)}
            >
              ×
            </button>
          </span>
        ))}
        <button
          ref={addButtonRef}
          type="button"
          className="workspace-secondary-action"
          aria-label="New remote tab"
          onClick={() => setNewTabOpen((open) => !open)}
        >
          +
        </button>
        {hostOnline ? null : <span className="workspace-remote-offline">offline</span>}
      </div>
      <WorkspaceNewTabMenu
        open={newTabOpen}
        triggerRef={addButtonRef}
        creating={creating}
        workspaceSelected
        showBrowser={false}
        onAgent={() => void handleNewAgent()}
        onTerminal={handleNewTerminal}
        onBrowser={() => setNewTabOpen(false)}
        onClose={() => setNewTabOpen(false)}
      />
      {providerPick === null ? null : (
        <div className="workspace-surface-menu" role="listbox" aria-label="Choose agent">
          <div className="workspace-menu-label">Choose agent</div>
          {providerPick.length === 0 ? (
            <div className="workspace-menu-label">No agents on this host.</div>
          ) : (
            providerPick.map((provider) => (
              <button
                key={provider.id}
                type="button"
                role="option"
                className="workspace-surface-option"
                onClick={() => startCreate("agent", provider)}
              >
                <span className="workspace-surface-name">{provider.id}</span>
              </button>
            ))
          )}
        </div>
      )}
      {createFailed === null ? null : (
        <div className="workspace-remote-create-failed" role="alert">
          <span className="workspace-menu-label">{createFailed.message}</span>
          <button
            type="button"
            className="workspace-secondary-action"
            onClick={retryCreate}
            disabled={creating}
          >
            Retry
          </button>
          <button
            type="button"
            className="workspace-secondary-action"
            onClick={dismissCreateFailed}
          >
            Dismiss
          </button>
        </div>
      )}
      {openSessionId === null ? null : streamState === "offline" || !hostOnline ? (
        <p className="workspace-remote-offline">offline</p>
      ) : openSession !== null && !openIsAgent ? (
        <RemoteTerminalPane
          key={`${deviceId}:${openSession.id}`}
          deviceId={deviceId}
          sessionId={openSession.id}
          title={openSession.title}
          attached={attached !== null && attached.sessionId === openSession.id ? attached : null}
          writersRef={terminalWritersRef}
          onArchive={
            attached !== null && attached.sessionId === openSession.id
              ? () => void archiveSession(openSession.id, attached.subscriptionId)
              : undefined
          }
          onClose={() => void closeSession(openSession.id)}
        />
      ) : (
        <div className="workspace-remote-transcript" role="log" aria-label="Remote transcript">
          {lines.map((line, index) => (
            <p key={index}>{line}</p>
          ))}
          {openSession === null || openSessionId === null ? null : (
            <div className="workspace-remote-pane">
              <div className="workspace-remote-pane-header">
                {attached !== null && attached.sessionId === openSessionId ? (
                  <>
                    <button
                      type="button"
                      className="workspace-secondary-action"
                      onClick={() =>
                        attached.sessionId === openSessionId &&
                        void archiveSession(openSessionId, attached.subscriptionId)
                      }
                    >
                      Archive
                    </button>
                    <button
                      type="button"
                      className="workspace-secondary-action"
                      onClick={() => openSessionId !== null && void closeSession(openSessionId)}
                    >
                      Close
                    </button>
                  </>
                ) : null}
              </div>
              {openCards.map((card) => (
                <PermissionCard
                  key={`${card.sessionId}:${card.request.toolCallId}`}
                  sessionId={card.sessionId}
                  subscriptionId={card.subscriptionId}
                  request={card.request}
                  capabilities={["typed_permissions"]}
                  daemonState={hostOnline ? "connected" : "disconnected"}
                  onRespond={answerCard(card)}
                />
              ))}
              <WorkspaceComposer
                streaming={streamState === "streaming"}
                turnActive={turnActive}
                queueAllowed={false}
                disabled={!sendReady}
                disabledReason={sendReady ? null : hostOnline ? "Connecting." : "offline"}
                onSend={async (text, attachments, fileReferences) => {
                  if (attached === null || attached.sessionId !== openSessionId) return false;
                  try {
                    await remoteHostSend({
                      deviceId,
                      sessionId: attached.sessionId,
                      subscriptionId: attached.subscriptionId,
                      text,
                      attachments,
                      attachmentReferences: fileReferences,
                      idempotencyKey: crypto.randomUUID(),
                    });
                    return true;
                  } catch {
                    return false;
                  }
                }}
                onStop={stopTurn}
              />
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * One terminal on the host, drawn by the reused terminal view. Keys travel
 * as sends on the stream's subscription, sizes as claim-then-resize, and
 * Ctrl+C as an interrupt — the same three roads the local surface drives,
 * pointed at the other machine.
 */
function RemoteTerminalPane({
  deviceId,
  sessionId,
  title,
  attached,
  writersRef,
  onArchive,
  onClose,
}: {
  deviceId: string;
  sessionId: string;
  title: string;
  attached: { sessionId: string; subscriptionId: number } | null;
  writersRef: React.MutableRefObject<Map<string, (data: string) => void>>;
  onArchive: (() => void) | undefined;
  onClose: () => void;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<TerminalViewHandle | null>(null);
  const attachedRef = useRef(attached);
  attachedRef.current = attached;

  useEffect(() => {
    const writers = writersRef.current;
    writers.set(sessionId, (data) => {
      viewRef.current?.write(data);
    });
    return () => {
      writers.delete(sessionId);
    };
  }, [sessionId, writersRef]);

  useEffect(() => {
    const host = hostRef.current;
    if (host === null) return;
    const view = createTerminalView(host, {
      onData: (data) => {
        const live = attachedRef.current;
        if (live === null || live.sessionId !== sessionId) return;
        void remoteHostSend({
          deviceId,
          sessionId,
          subscriptionId: live.subscriptionId,
          text: data,
        }).catch(() => undefined);
      },
      onCtrlC: () => {
        const live = attachedRef.current;
        if (live === null || live.sessionId !== sessionId) return;
        void remoteHostInterrupt(deviceId, sessionId, live.subscriptionId).catch(() => undefined);
      },
    });
    viewRef.current = view;
    const observer =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(() => {
            const live = attachedRef.current;
            if (live === null || live.sessionId !== sessionId) return;
            if (!view.fit()) return;
            const cols = view.cols();
            const rows = view.rows();
            if (cols <= 0 || rows <= 0) return;
            void remoteHostResize(deviceId, sessionId, live.subscriptionId, cols, rows).catch(
              () => undefined,
            );
          });
    const element = hostRef.current;
    if (observer !== null && element !== null) observer.observe(element);
    return () => {
      observer?.disconnect();
      view.dispose();
      viewRef.current = null;
    };
  }, [deviceId, sessionId]);

  // Claim at attach makes the first resize deterministic: the pane fits the
  // grid it is seen on instead of the daemon's default. Runs when the
  // stream is up, not when the view mounts — the view mounts first and the
  // attach lands after, so claiming on mount would race it and be refused.
  useEffect(() => {
    if (attached === null || attached.sessionId !== sessionId) return;
    const subscriptionId = attached.subscriptionId;
    const view = viewRef.current;
    if (view === null) return;
    void (async () => {
      try {
        await remoteHostClaim(deviceId, sessionId, subscriptionId);
        if (attachedRef.current?.subscriptionId !== subscriptionId) return;
        if (view.fit()) {
          const cols = view.cols();
          const rows = view.rows();
          if (cols > 0 && rows > 0) {
            await remoteHostResize(deviceId, sessionId, subscriptionId, cols, rows);
          }
        }
      } catch {
        // The roster says whether it went; a miss here is not a pane error.
      }
    })();
  }, [attached, deviceId, sessionId]);

  return (
    <div className="workspace-remote-terminal">
      <div className="workspace-remote-pane-header">
        <span className="workspace-menu-label">{title.trim() === "" ? sessionId : title}</span>
        {onArchive === undefined ? null : (
          <button type="button" className="workspace-secondary-action" onClick={onArchive}>
            Archive
          </button>
        )}
        <button type="button" className="workspace-secondary-action" onClick={onClose}>
          Close
        </button>
      </div>
      <div ref={hostRef} className="workspace-remote-terminal-view" />
    </div>
  );
}
