/**
 * What Enter does while the agent works — the app's "Enter while the agent
 * runs" choice, the same three choices Paseo offers (`interrupt`, `steer`,
 * `queue`; packages/app/src/hooks/use-settings/storage.ts:219). Paseo defaults
 * to steer (storage.ts:141): Enter joins the running turn, and the queue
 * button, or Command/Ctrl+Enter, queues behind it. One JSON value in
 * localStorage, best-effort both ways, the same pattern as the model-effort
 * preference: no daemon round-trip for a preference only this app reads.
 *
 * The stored words are this setting's own vocabulary, deliberately not the
 * protocol's: `steer-and-send` and `interrupt-and-send` name what Enter does,
 * and `activeTurnBehaviorFor` maps them onto the wire's words.
 */
export type SendBehavior = "steer-and-send" | "interrupt-and-send" | "queue";

/** The setting's values at runtime, so a test (or a future reader) can hold
 * the stored vocabulary against the wire's words without retyping it. */
export const SEND_BEHAVIOR_VALUES: readonly SendBehavior[] = [
  "steer-and-send",
  "interrupt-and-send",
  "queue",
];

const STORAGE_KEY = "devboule.sendBehavior";

/** The stored values an earlier build wrote, and the words they become. The
 * old `steer` meant interrupt-and-send, so it keeps that meaning. Reading is
 * where an old value is carried over, so the old word is gone for good the
 * first time the setting is touched. */
const MIGRATED_VALUES: Record<string, SendBehavior> = {
  steer: "interrupt-and-send",
};

function readStoredValue(): SendBehavior {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    // A boot with nothing stored takes Paseo's default: steer.
    if (raw === null) return "steer-and-send";
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed === "string" && SEND_BEHAVIOR_VALUES.includes(parsed as SendBehavior)) {
      return parsed as SendBehavior;
    }
    if (typeof parsed === "string" && MIGRATED_VALUES[parsed] !== undefined) {
      const migrated = MIGRATED_VALUES[parsed];
      localStorage.setItem(STORAGE_KEY, JSON.stringify(migrated));
      return migrated;
    }
    return "steer-and-send";
  } catch {
    // Corrupt or unavailable storage means the default, never a broken chat.
    return "steer-and-send";
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
 * parked until the request is answered, so a queued send behind an open
 * permission card resolves to interrupt-and-send instead (Paseo:
 * packages/app/src/composer/input/state.ts:9-12).
 */
export function resolveActiveSendBehavior(
  sendBehavior: SendBehavior,
  hasPendingPermission: boolean,
): SendBehavior {
  return sendBehavior === "queue" && hasPendingPermission ? "interrupt-and-send" : sendBehavior;
}

/** The wire's active-turn word for a send made while a turn runs. Send does
 * what the setting says, and the queue setting's send is the interrupt, as
 * Paseo's alternate submit does under queue (composer/index.tsx:1506). */
export function activeTurnBehaviorFor(sendBehavior: SendBehavior): "steer" | "interrupt" {
  return sendBehavior === "steer-and-send" ? "steer" : "interrupt";
}

export type ComposerActionLabel = "Queue message" | "Send and steer" | "Send and interrupt";

/** The submit-button words on the button that does what Enter does while
 * the turn runs: queue, steer, or interrupt — the label names the act. */
export function composerActionLabel(
  defaultActionQueues: boolean,
  sendWhileRunning: "steer" | "interrupt" = "interrupt",
): ComposerActionLabel {
  if (defaultActionQueues) return "Queue message";
  return sendWhileRunning === "steer" ? "Send and steer" : "Send and interrupt";
}
