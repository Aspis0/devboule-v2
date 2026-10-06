import { useCallback, useEffect, useSyncExternalStore } from "react";
import type { AgentFinished } from "../../../lib/agentSession";
import type { ContextUsage, SessionManifest } from "../../../types/ipc";
import { publishAgentReading, retireAgentReading } from "./agentReadingStore";

/** What the publisher needs of the session: its own slice of the store, with
    its own subscribers — the transcript surface never re-renders for a reading. */
export interface UsageSource {
  subscribeUsage(listener: () => void): () => void;
  getContextUsage(): ContextUsage | null;
}

interface AgentReadingPublisherProps {
  sessionId: string;
  session: UsageSource | null;
  manifest: SessionManifest | null;
  lastFinished: AgentFinished | null;
  task: string | null;
}

/**
 * Publishes the surface's reading for the status bar and draws nothing. It
 * re-renders when a context reading arrives; the surface that mounts it does
 * not.
 */
export function AgentReadingPublisher({
  sessionId,
  session,
  manifest,
  lastFinished,
  task,
}: AgentReadingPublisherProps) {
  const subscribe = useCallback(
    (listener: () => void) => session?.subscribeUsage(listener) ?? (() => {}),
    [session],
  );
  const read = useCallback(() => session?.getContextUsage() ?? null, [session]);
  const usage = useSyncExternalStore(subscribe, read);
  useEffect(() => {
    publishAgentReading(sessionId, { usage, manifest, lastFinished, task });
  }, [sessionId, usage, manifest, lastFinished, task]);
  useEffect(() => () => retireAgentReading(sessionId), [sessionId]);
  return null;
}
