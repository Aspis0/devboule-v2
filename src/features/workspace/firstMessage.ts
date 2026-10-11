/** What the first message needs from a new agent's controller: its state, its
 * change feed, and the send itself. `AgentSession` satisfies it. */
export interface FirstMessageAgent {
  getState(): { status: string };
  subscribe(listener: () => void): () => void;
  send(text: string): Promise<boolean>;
}

/** The status an agent reports once its attach has completed and it is ready. */
const READY_STATUS = "idle";

/**
 * Sends a new agent's first message once it is attached and idle: a controller
 * refuses a send before its attach completes, so the message waits for the
 * status change that says it is ready. Sends at most once. The returned
 * function drops the wait when the surface goes away first.
 */
export function sendFirstMessageWhenReady(agent: FirstMessageAgent, text: string): () => void {
  let settled = false;
  let unsubscribe: (() => void) | null = null;
  const trySend = (): boolean => {
    if (settled || agent.getState().status !== READY_STATUS) return false;
    settled = true;
    unsubscribe?.();
    void agent.send(text);
    return true;
  };
  if (trySend()) return () => undefined;
  unsubscribe = agent.subscribe(() => {
    trySend();
  });
  return () => {
    settled = true;
    unsubscribe?.();
  };
}
