import { useSyncExternalStore } from "react";
import type { AgentFinished } from "../../../lib/agentSession";
import type { ContextUsage, SessionManifest } from "../../../types/ipc";

/**
 * What an agent surface knows that the status bar shows for the focused agent:
 * the context reading, the manifest that names its window, the last finished
 * turn, and the task the agent is on. Published by the surface that owns the
 * session, read by the bar, which holds no controller of its own.
 */
export interface AgentReading {
  usage: ContextUsage | null;
  manifest: SessionManifest | null;
  lastFinished: AgentFinished | null;
  /** The in-progress step of the agent's plan, in its own words. */
  task: string | null;
}

const readings = new Map<string, AgentReading>();
const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of listeners) listener();
}

export function publishAgentReading(sessionId: string, reading: AgentReading): void {
  readings.set(sessionId, reading);
  notify();
}

/** The surface is gone: its reading must not outlive the session it described. */
export function retireAgentReading(sessionId: string): void {
  if (readings.delete(sessionId)) notify();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The session's latest published reading, or null while no surface publishes one. */
export function useAgentReading(sessionId: string | null): AgentReading | null {
  return useSyncExternalStore(subscribe, () =>
    sessionId === null ? null : (readings.get(sessionId) ?? null),
  );
}
