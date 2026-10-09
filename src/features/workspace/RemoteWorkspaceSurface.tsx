import { useCallback, useEffect, useRef, useState } from "react";
import {
  createRemoteEventChannel,
  remoteHostList,
  remoteSessionAttach,
  remoteSessionDetach,
  type RemoteEventChannel,
} from "../../lib/tauri";
import type { RemoteRelayedEvent, Session, SessionEvent } from "../../types/ipc";

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
  const subscriptionRef = useRef(0);
  const openRef = useRef<string | null>(null);

  // The host's own session rows for this workspace, reloaded on the online
  // edge so a reconnect shows the roster as it is now.
  useEffect(() => {
    let live = true;
    void (async () => {
      try {
        const body = await remoteHostList(deviceId, { kind: "sessions" });
        if (!live || body.list !== "sessions") return;
        setSessions(body.rows.filter((row) => row.workspaceId === workspaceId));
      } catch {
        // Keep the last list; the row's own offline state says why.
      }
    })();
    return () => {
      live = false;
    };
  }, [deviceId, hostOnline, workspaceId]);

  // Open the session's stream, reattaching with a fresh subscription on the
  // online edge. A subscription from an older edge is fenced by the daemon,
  // so the events of the replaced stream can never land here.
  useEffect(() => {
    const previous = openRef.current;
    openRef.current = openSessionId;
    if (previous !== null && previous !== openSessionId) {
      const previousSubscription = subscriptionRef.current;
      void remoteSessionDetach(deviceId, previous, previousSubscription).catch(() => undefined);
    }
    if (openSessionId === null) {
      setStreamState("idle");
      return;
    }
    if (!hostOnline) {
      setStreamState("offline");
      return;
    }
    subscriptionRef.current += 1;
    const subscriptionId = subscriptionRef.current;
    setLines([]);
    setStreamState("streaming");
    const channel: RemoteEventChannel = createRemoteEventChannel((event: RemoteRelayedEvent) => {
      if (event.deviceId !== deviceId || event.sessionId !== openSessionId) return;
      if (event.subscriptionId !== subscriptionId) return;
      const line = lineOf(event.envelope.event);
      if (line !== null) setLines((current) => [...current, line]);
    });
    void remoteSessionAttach(deviceId, openSessionId, subscriptionId, channel).catch(() => {
      setStreamState("offline");
    });
  }, [deviceId, hostOnline, openSessionId]);

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
