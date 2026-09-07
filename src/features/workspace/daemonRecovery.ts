import { ask } from "@tauri-apps/plugin-dialog";
import { daemonRestart } from "../../lib/tauri";
import type { DaemonStatus } from "../../types/ipc";

/**
 * Recovery for a daemon that is alive but no longer answering.
 *
 * The supervisor detects the wedge and reports `unresponsive`; restarting is
 * destructive (it closes the Job Object owning every agent and terminal
 * process, so live turns die with it — transcripts survive in the journal).
 * The decision therefore lives in the frontend, which still holds the roster:
 *
 * - nothing live at stake: restart straight away, silently;
 * - anything live: ask first, with plain wording about what stops and what is
 *   kept.
 *
 * Suppression policy is NOT duplicated here — the daemon decides what counts
 * as unresponsive and says so in the status; this module only decides whether
 * to act on it, and only once per episode.
 */
export interface DaemonRecoveryDeps {
  restart: () => Promise<unknown>;
  ask: (
    message: string,
    options?: {
      title?: string;
      kind?: "info" | "warning" | "error";
      okLabel?: string;
      cancelLabel?: string;
    },
  ) => Promise<boolean>;
}

const CONFIRM_MESSAGE =
  "The daemon has stopped answering. Restarting it will stop the agents that are " +
  "running right now — your conversations are kept and can be reopened. Restart now?";

/** Shown in the status strip while the daemon is still unresponsive after a restart attempt that failed. */
const ATTEMPT_FAILED_NOTE = "a restart was attempted, but it did not complete";

export interface DaemonRecovery {
  onStatus(status: DaemonStatus): void;
  /**
   * Pushes the current roster answer from the frontend. The module holds the
   * latest value, so the caller never needs a render-time ref read.
   */
  setRoster(hasLiveSession: boolean): void;
  /** Subscribe to note changes (for `useSyncExternalStore`). */
  subscribe(listener: () => void): () => void;
  /**
   * The attempt-failed sentence for the current episode, or null. Both facts
   * hold at once while the daemon is unresponsive: the daemon's own message
   * and this note. The note survives unresponsive polls without flickering
   * and clears the moment any other status arrives, like the episode.
   */
  note(): string | null;
}

/**
 * An episode is a maximal run of consecutive `unresponsive` statuses: it
 * begins with the first unresponsive report and ends when any other state
 * arrives (a fresh daemon, a disconnect, an error). Within an episode the
 * user is asked at most once; while the dialog is open, further polls change
 * nothing. After a decline the state stays visible in the daemon status strip
 * until the daemon recovers or the app restarts.
 */
export function createDaemonRecovery(overrides: Partial<DaemonRecoveryDeps> = {}): DaemonRecovery {
  const restart = overrides.restart ?? (() => daemonRestart());
  const askDialog =
    overrides.ask ??
    ((
      message: string,
      options?: {
        title?: string;
        kind?: "info" | "warning" | "error";
        okLabel?: string;
        cancelLabel?: string;
      },
    ) => ask(message, options));
  // Direction of failure: assume a live session exists. Restarting is the
  // destructive act — it kills every running agent's turn — so consent is the
  // thing that must never be skipped. If the roster wiring (setRoster) is
  // ever dropped or refactored away, this default degrades to an unnecessary
  // question, which is annoying but safe; the opposite default would silently
  // destroy live work.
  let hasLiveSession = true;

  let episodeHandled = false;
  let dialogOpen = false;
  let attemptFailed = false;
  let lastStatusState: DaemonStatus["state"] | null = null;
  const listeners = new Set<() => void>();
  const notify = (): void => {
    for (const listener of listeners) listener();
  };

  // One shared restart path so a rejection marks the attempt failed wherever
  // it was started from. The episode stays consumed: one attempt per episode,
  // and the note is how the user learns the attempt did not complete.
  const runRestart = (): void => {
    void restart().then(
      () => undefined,
      () => {
        attemptFailed = true;
        notify();
      },
    );
  };

  return {
    setRoster(hasLive: boolean): void {
      hasLiveSession = hasLive;
    },

    onStatus(status: DaemonStatus): void {
      if (status.state !== "unresponsive") {
        episodeHandled = false;
        lastStatusState = status.state;
        if (attemptFailed) {
          attemptFailed = false;
          notify();
        }
        return;
      }
      lastStatusState = "unresponsive";
      if (episodeHandled || dialogOpen) return;
      episodeHandled = true;

      if (!hasLiveSession) {
        runRestart();
        return;
      }
      dialogOpen = true;
      void askDialog(CONFIRM_MESSAGE, {
        title: "Daemon not answering",
        kind: "warning",
        okLabel: "Restart daemon",
        cancelLabel: "Wait",
      })
        .then((confirmed) => {
          dialogOpen = false;
          if (confirmed) runRestart();
        })
        .catch(() => {
          dialogOpen = false;
        });
    },

    subscribe(listener: () => void): () => void {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },

    note(): string | null {
      return lastStatusState === "unresponsive" && attemptFailed ? ATTEMPT_FAILED_NOTE : null;
    },
  };
}
