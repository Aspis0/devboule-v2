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
 * Sends a message only when the agent is attached and idle: a controller
 * refuses a send before its attach completes. Resolves with what the agent
 * answered; a send that is not ready resolves false without being attempted.
 */
export async function sendWhenReady(agent: FirstMessageAgent, text: string): Promise<boolean> {
  if (agent.getState().status !== READY_STATUS) return false;
  try {
    return await agent.send(text);
  } catch {
    return false;
  }
}

/**
 * Sends a new agent's first message once it is attached and idle, and reports
 * the answer once. A message is never sent twice by this wait: when the agent
 * answers, the wait ends. The returned function drops the wait when the
 * surface goes away first; a dropped wait reports nothing, so the caller
 * keeps the text.
 */
export function sendFirstMessageWhenReady(
  agent: FirstMessageAgent,
  text: string,
  onSettled: (sent: boolean) => void,
): () => void {
  let settled = false;
  let unsubscribe: (() => void) | null = null;
  const trySend = (): boolean => {
    if (settled || agent.getState().status !== READY_STATUS) return false;
    settled = true;
    unsubscribe?.();
    void sendWhenReady(agent, text).then(onSettled);
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
