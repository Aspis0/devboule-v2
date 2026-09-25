/**
 * What Enter does while the agent works — the app's "Default send" choice:
 * queue the message, or interrupt the running turn and send it as a new turn
 * (the UI's "Steer"). One JSON value in localStorage, best-effort both ways,
 * the same pattern as the model-effort preference: no daemon round-trip for a
 * preference only this app reads.
 *
 * The stored words are this setting's own vocabulary, deliberately not the
 * protocol's: a `session_send` takes `activeTurnBehavior: "steer"` (delivered
 * into the running turn — never what this setting means), and `"queue"` is a
 * word the wire does not know. Nothing stored here can be handed to a send and
 * look like a behaviour the daemon never agreed to.
 */
export type SendBehavior = "queue" | "interrupt-and-send";

/** The setting's values at runtime, so a test (or a future reader) can hold
 * the stored vocabulary against the wire's words without retyping it. */
export const SEND_BEHAVIOR_VALUES: readonly SendBehavior[] = ["queue", "interrupt-and-send"];

const STORAGE_KEY = "devboule.sendBehavior";

/** The one value an earlier build stored for interrupt-and-send, and the word
 * it becomes. Reading is where an old value is carried over, so the old word
 * is gone for good the first time the setting is touched. */
const MIGRATED_VALUES: Record<string, SendBehavior> = {
  steer: "interrupt-and-send",
};

function readStoredValue(): SendBehavior {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw === null) return "queue";
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed === "string" && SEND_BEHAVIOR_VALUES.includes(parsed as SendBehavior)) {
      return parsed as SendBehavior;
    }
    if (typeof parsed === "string" && MIGRATED_VALUES[parsed] !== undefined) {
      const migrated = MIGRATED_VALUES[parsed];
      localStorage.setItem(STORAGE_KEY, JSON.stringify(migrated));
      return migrated;
    }
    return "queue";
  } catch {
    // Corrupt or unavailable storage means the default, never a broken chat.
    return "queue";
  }
}

let current: SendBehavior = readStoredValue();
const listeners = new Set<(value: SendBehavior) => void>();

/** A cached read: the store is the only thing that touches localStorage. */
export function getSendBehavior(): SendBehavior {
  return current;
}

export function setSendBehavior(value: SendBehavior): void {
  if (value === current) return;
  current = value;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(value));
  } catch {
    // Storage can be full or blocked; a lost preference must not break the chat.
  }
  for (const listener of listeners) listener(current);
}

export function subscribeSendBehavior(listener: (value: SendBehavior) => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/**
 * Queueing behind a permission prompt would strand the message: the turn is
 * parked until the request is answered. Paseo's `resolveActiveSendBehavior`,
 * resolved to our interrupt-and-send instead of its interrupt.
 */
export function resolveActiveSendBehavior(
  sendBehavior: SendBehavior,
  hasPendingPermission: boolean,
): SendBehavior {
  return sendBehavior === "queue" && hasPendingPermission ? "interrupt-and-send" : sendBehavior;
}

export type ComposerActionLabel = "Queue message" | "Send and interrupt";

/** Paseo's submit-button words on the button that does what Enter does while
 * the turn runs: queue, or interrupt-and-send — the label says it interrupts. */
export function composerActionLabel(defaultActionQueues: boolean): ComposerActionLabel {
  return defaultActionQueues ? "Queue message" : "Send and interrupt";
}
