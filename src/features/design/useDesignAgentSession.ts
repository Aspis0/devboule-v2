import { useCallback, useEffect, useRef, useState } from "react";
import type {
  DesignAgentSession,
  DesignHost,
  DesignTranscriptItem,
  PendingPermission,
} from "./designHost";
import type { AgentSessionState } from "../../lib/agentSession";
import type { Session } from "../../types/ipc";
import { transcriptItems } from "./agentHost";
import { EMPTY_TRANSCRIPT } from "./designMessageModel";
import type { PermissionAnswer } from "../../components/PermissionCard";
import { clearDelegatedMirrorPin } from "./delegatedDesignMirror";

interface UseDesignAgentSessionInput {
  host: DesignHost;
  busy: boolean;
}

interface UseDesignAgentSessionResult {
  agentSession: DesignAgentSession | null;
  agentState: AgentSessionState | null;
  agentSessionRecord: Session | null;
  pendingPermission: PendingPermission | null;
  permissionNotice: string | null;
  streamingTranscript: readonly DesignTranscriptItem[];
  streamingTranscriptRef: { current: readonly DesignTranscriptItem[] };
  liveSessionIdRef: { current: string | null };
  setPermissionNotice: (notice: string | null) => void;
  selectModel: (modelId: string) => void;
  selectEffort: (effort: string) => void;
  respondPermission: (response: PermissionAnswer) => Promise<void>;
  endSession: () => void;
}

export function useDesignAgentSession(
  input: UseDesignAgentSessionInput,
): UseDesignAgentSessionResult {
  const [agentSession, setAgentSession] = useState<DesignAgentSession | null>(
    () => input.host.getAgentSession?.() ?? null,
  );
  const [agentState, setAgentState] = useState<AgentSessionState | null>(
    () => input.host.getAgentSession?.()?.getState() ?? null,
  );
  const [agentSessionRecord, setAgentSessionRecord] = useState<Session | null>(
    () => input.host.getAgentSessionRecord?.() ?? null,
  );
  const [pendingPermission, setPendingPermission] = useState<PendingPermission | null>(
    () => input.host.getPendingPermission?.() ?? null,
  );
  const [permissionNotice, setPermissionNotice] = useState<string | null>(
    () => input.host.getPermissionNotice?.() ?? null,
  );
  const liveSessionIdRef = useRef<string | null>(agentSessionRecord?.id ?? null);

  useEffect(() => {
    const updateAgentSession = (): void => {
      const next = input.host.getAgentSession?.() ?? null;
      const nextRecord = input.host.getAgentSessionRecord?.() ?? null;
      // An absent record means this host has no live session to protect from a history attach.
      liveSessionIdRef.current = nextRecord?.id ?? null;
      setAgentSession(next);
      setAgentState(next?.getState() ?? null);
      setAgentSessionRecord(nextRecord);
      setPendingPermission(input.host.getPendingPermission?.() ?? null);
      setPermissionNotice(input.host.getPermissionNotice?.() ?? null);
    };
    const unsubscribe = input.host.subscribeAgentSession?.(updateAgentSession);
    updateAgentSession();
    return () => unsubscribe?.();
  }, [input.host]);

  const [streamingTranscript, setStreamingTranscript] =
    useState<readonly DesignTranscriptItem[]>(EMPTY_TRANSCRIPT);
  // Read by DesignSurface's startGeneration failure path and its stop/retry actions.
  // Those callbacks must see the rows live at the moment they run, not the
  // rows live at the moment they were created, so they read this ref instead
  // of taking streamingTranscript as a dependency (which would recreate them
  // on every stream chunk).
  const streamingTranscriptRef = useRef(streamingTranscript);

  useEffect(() => {
    if (agentSession === null) return;
    // This subscription is the surface's single live view of the session, and it also keeps the
    // transcript rows. The boundary comes from the host because it is recorded after the
    // craft-selection pre-flight, whose items are the host's own question, not the agent's
    // answer. Stop and failure run outside render, so they read the same rows the working card
    // renders instead of taking a dependency on every chunk.
    const update = (): void => {
      const state = agentSession.getState();
      setAgentState(state);
      const start = input.host.getRunTranscriptStart?.() ?? null;
      const next = start === null ? EMPTY_TRANSCRIPT : transcriptItems(state.items, start);
      streamingTranscriptRef.current = next;
      setStreamingTranscript(next);
    };
    update();
    return agentSession.subscribe(update);
  }, [agentSession, input.host]);

  const selectModel = useCallback(
    (modelId: string) => {
      void agentSession?.setModel(modelId);
    },
    [agentSession],
  );
  const selectEffort = useCallback(
    (effort: string) => {
      void agentSession?.setModel(undefined, effort);
    },
    [agentSession],
  );
  const respondPermission = useCallback(
    // The card's answer object travels whole to the host: this layer names
    // no fields, so a new answer carrier cannot be dropped here.
    (response: PermissionAnswer): Promise<void> =>
      input.host.respondPermission?.(response) ?? Promise.resolve(),
    [input.host],
  );
  const endSession = useCallback(() => {
    if (input.busy || agentSession === null) return;
    // Ending the session closes the reading too: the pin goes with it, so
    // the next delegation may mirror again.
    clearDelegatedMirrorPin();
    void input.host.closeAgentSession?.();
  }, [agentSession, input.busy, input.host]);

  return {
    agentSession,
    agentState,
    agentSessionRecord,
    pendingPermission,
    permissionNotice,
    streamingTranscript,
    streamingTranscriptRef,
    liveSessionIdRef,
    setPermissionNotice,
    selectModel,
    selectEffort,
    respondPermission,
    endSession,
  };
}
