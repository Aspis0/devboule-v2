import { useCallback, useEffect, useRef, useState } from "react";
import {
  allocRemoteSubscriptionId,
  createRemoteEventChannel,
  remoteHostList,
  remoteSessionAttach,
  remoteSessionDetach,
  type RemoteEventChannel,
} from "../../lib/tauri";
import type { RemoteRelayMessage, Session, SessionEvent } from "../../types/ipc";

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

/**
 * The remote workspace's surface: the host's sessions for this workspace as
 * tabs and the opened one's transcript. Read-only by design — every frame
 * comes from the owning daemon, sending is the next slice — and while the
 * host is gone the tab row says one short word and the stream resumes on the
 * next online edge with a fresh subscription.
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
      const channel: RemoteEventChannel = createRemoteEventChannel(
        (message: RemoteRelayMessage) => {
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
          const line = lineOf(message.envelope.event);
          if (line !== null) appendLine(line);
          // A state, a terminal exit or a permission card changes what the
          // tabs should say; the roster is re-read rather than left stale,
          // coalesced so a burst of events is one read.
          if (!isBulkTranscript(message.envelope.event)) requestLoad();
        },
      );
      await remoteSessionAttach(deviceId, target, subscriptionId, channel).catch(() => {
        if (token === operationRef.current) setStreamState("offline");
      });
    });
    chainRef.current = run;
    return () => {
      if (reattachTimerRef.current !== null) {
        clearTimeout(reattachTimerRef.current);
        reattachTimerRef.current = null;
      }
    };
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

  return (
    <div className="workspace-remote-surface">
      <div className="workspace-remote-tabs" role="tablist" aria-label="Remote sessions">
        {sessions.map((session) => (
          <button
            key={session.id}
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
        ))}
        {hostOnline ? null : <span className="workspace-remote-offline">offline</span>}
      </div>
      {openSessionId === null ? null : streamState === "offline" || !hostOnline ? (
        <p className="workspace-remote-offline">offline</p>
      ) : (
        <div className="workspace-remote-transcript" role="log" aria-label="Remote transcript">
          {lines.map((line, index) => (
            <p key={index}>{line}</p>
          ))}
        </div>
      )}
    </div>
  );
}
