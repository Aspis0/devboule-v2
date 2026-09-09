import {
  AgentSession,
  type AgentSessionDeps,
  type AgentSessionState,
} from "../../lib/agentSession";
import {
  createSessionChannel,
  sessionAttach,
  sessionDetach,
  type SessionChannel,
} from "../../lib/tauri";
import { extractArtifact } from "./agentHost";

export const DESIGN_HISTORY_OPEN_TIMEOUT_MS = 5_000;
// This is a quiescence window for replay chunks, not a deadline for opening history.
export const DESIGN_HISTORY_OPEN_QUIET_MS = 400;

export type DesignHistoryOpenResult =
  | { status: "loading" }
  | { status: "artifact"; html: string }
  | { status: "timeout"; message: string }
  | { status: "failed"; message: string };

export interface DesignHistoryOpenDeps {
  onResult: (result: DesignHistoryOpenResult) => void;
  invoke?: AgentSessionDeps["invoke"];
  createChannel?: AgentSessionDeps["createChannel"];
  timeoutMs?: number;
  setTimeout?: typeof globalThis.setTimeout;
  clearTimeout?: typeof globalThis.clearTimeout;
}

export interface DesignHistoryOpenHandle {
  dispose: () => void;
}

function reasonFromCause(cause: unknown): string {
  if (cause instanceof Error && cause.message) return cause.message;
  if (typeof cause === "string" && cause.trim()) return cause;
  if (typeof cause === "object" && cause !== null && "message" in cause) {
    const message = cause.message;
    if (typeof message === "string" && message.trim()) return message;
  }
  return "The transcript could not be opened.";
}

function lastErrorText(state: AgentSessionState): string {
  for (let index = state.items.length - 1; index >= 0; index -= 1) {
    const item = state.items[index];
    if (item.role === "error") return item.text;
  }
  return "The transcript could not be opened.";
}

function timeoutMessage(timeoutMs: number): string {
  const seconds = timeoutMs / 1_000;
  const formattedSeconds = Number.isInteger(seconds)
    ? String(seconds)
    : seconds.toFixed(2).replace(/0+$/, "").replace(/\.$/, "");
  const noun = formattedSeconds === "1" ? "second" : "seconds";
  return `The transcript did not produce a design within ${formattedSeconds} ${noun}.`;
}

/**
 * Keep the reopen controller's command vocabulary narrower than the general agent host. This
 * rejects an accidental resume or close before either command can reach the injected bridge.
 */
export function createReadOnlyHistoryInvoke(
  invoke: AgentSessionDeps["invoke"],
): AgentSessionDeps["invoke"] {
  return <T>(command: string, args?: Record<string, unknown>): Promise<T> => {
    switch (command) {
      case "session_attach":
      case "session_detach":
        return invoke<T>(command, args);
      default:
        return Promise.reject(new Error(`Unsupported design history command: ${command}`));
    }
  };
}

const tauriInvoke: AgentSessionDeps["invoke"] = <T>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> => {
  switch (command) {
    case "session_attach":
      return sessionAttach(
        args.id as string,
        // An absent cursor means hydrate the transcript from its beginning.
        (args.fromCursor as number | null | undefined) ?? null,
        args.ch as SessionChannel,
      ) as Promise<T>;
    case "session_detach":
      return sessionDetach(args.subscriptionId as number) as Promise<T>;
    default:
      return Promise.reject(new Error(`Unsupported design history command: ${command}`));
  }
};

export function openDesignHistoryEntry(
  sessionId: string,
  deps: DesignHistoryOpenDeps,
): DesignHistoryOpenHandle {
  // An absent timeout means the standard five-second window for a journal replay.
  const timeoutMs = Math.max(0, deps.timeoutMs ?? DESIGN_HISTORY_OPEN_TIMEOUT_MS);
  // An absent timer dependency means use the browser clock; fake clocks belong in tests.
  const setTimer = deps.setTimeout ?? globalThis.setTimeout;
  const clearTimer = deps.clearTimeout ?? globalThis.clearTimeout;
  const controller = new AgentSession({
    sessionId,
    // An absent invoke is the production bridge; tests inject a bridge to observe the boundary.
    invoke: createReadOnlyHistoryInvoke(deps.invoke ?? tauriInvoke),
    // An absent channel factory means use the real Tauri event channel.
    createChannel: deps.createChannel ?? createSessionChannel,
  });

  let disposed = false;
  let timer: ReturnType<typeof globalThis.setTimeout> | null = null;
  let quietTimer: ReturnType<typeof globalThis.setTimeout> | null = null;
  let artifactObserved = false;
  let unsubscribe = (): void => undefined;

  const dispose = (): void => {
    if (disposed) return;
    disposed = true;
    if (timer !== null) {
      clearTimer(timer);
      timer = null;
    }
    if (quietTimer !== null) {
      clearTimer(quietTimer);
      quietTimer = null;
    }
    unsubscribe();
    // AgentSession sequences this detach after an attach that is still in flight, which keeps
    // the journal pin balanced even when the surface disappears during transcript hydration.
    controller.dispose();
  };

  const finish = (result: DesignHistoryOpenResult): void => {
    if (disposed) return;
    try {
      deps.onResult(result);
    } finally {
      dispose();
    }
  };

  const settleLatestArtifact = (): boolean => {
    const extraction = extractArtifact(controller.getState());
    if (extraction.html !== undefined) {
      finish({ status: "artifact", html: extraction.html });
      return true;
    }
    if (extraction.error !== undefined) {
      finish({ status: "failed", message: extraction.error });
      return true;
    }
    return false;
  };

  const restartQuietTimer = (): void => {
    if (quietTimer !== null) clearTimer(quietTimer);
    quietTimer = setTimer(() => {
      quietTimer = null;
      if (disposed) return;
      if (!settleLatestArtifact() && artifactObserved) {
        finish({ status: "failed", message: "The transcript could not be opened." });
      }
    }, DESIGN_HISTORY_OPEN_QUIET_MS);
  };

  const inspect = (): void => {
    if (disposed) return;
    try {
      const state = controller.getState();
      const extraction = extractArtifact(state);
      if (extraction.html !== undefined || extraction.error !== undefined) {
        artifactObserved = true;
        restartQuietTimer();
        return;
      }

      if (state.status === "error") {
        finish({ status: "failed", message: lastErrorText(state) });
      }
    } catch (cause) {
      finish({ status: "failed", message: reasonFromCause(cause) });
    }
  };

  deps.onResult({ status: "loading" });
  unsubscribe = controller.subscribe(inspect);
  timer = setTimer(() => {
    if (artifactObserved && !settleLatestArtifact()) {
      finish({ status: "failed", message: "The transcript could not be opened." });
      return;
    }
    finish({ status: "timeout", message: timeoutMessage(timeoutMs) });
  }, timeoutMs);
  void controller.start().catch((cause: unknown) => {
    finish({ status: "failed", message: reasonFromCause(cause) });
  });

  return { dispose };
}
