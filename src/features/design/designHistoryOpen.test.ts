import { describe, expect, it, vi } from "vitest";
import type { AgentChannel, AgentSessionDeps } from "../../lib/agentSession";
import type { SessionEvent } from "../../types/ipc";
import { ARTIFACT_TOO_LARGE_MESSAGE, MAX_ARTIFACT_BYTES } from "./agentHost";
import {
  createReadOnlyHistoryInvoke,
  DESIGN_HISTORY_OPEN_QUIET_MS,
  openDesignHistoryEntry,
  type DesignHistoryOpenDeps,
  type DesignHistoryOpenResult,
} from "./designHistoryOpen";

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
} {
  let resolvePromise: ((value: T) => void) | undefined;
  const promise = new Promise<T>((resolve) => {
    resolvePromise = resolve;
  });
  return {
    promise,
    resolve: (value: T) => resolvePromise?.(value),
  };
}

function historyHarness(
  invoke: AgentSessionDeps["invoke"],
  options: Pick<DesignHistoryOpenDeps, "clearTimeout" | "setTimeout" | "timeoutMs"> = {},
) {
  let emit: ((event: SessionEvent) => void) | null = null;
  const results: DesignHistoryOpenResult[] = [];
  const handle = openDesignHistoryEntry("history-session", {
    invoke,
    createChannel: (onEvent) => {
      emit = onEvent;
      return {} as AgentChannel;
    },
    onResult: (result) => results.push(result),
    timeoutMs: options.timeoutMs ?? 5_000,
    setTimeout: options.setTimeout,
    clearTimeout: options.clearTimeout,
  });
  return { emit: (event: SessionEvent) => emit?.(event), handle, results };
}

describe("design history reopen", () => {
  it("rejects session_resume and session_close before the bridge can receive them", async () => {
    const bridge = vi.fn(async () => undefined) as unknown as AgentSessionDeps["invoke"];
    const invoke = createReadOnlyHistoryInvoke(bridge);

    await expect(invoke("session_resume", { sessionId: "history-session" })).rejects.toThrow(
      "Unsupported design history command",
    );
    await expect(invoke("session_close", { id: "history-session" })).rejects.toThrow(
      "Unsupported design history command",
    );
    expect(bridge).not.toHaveBeenCalled();
  });

  it("detaches exactly once when disposed while attach is in flight", async () => {
    const attach = deferred<number>();
    const bridge = vi.fn(async (command: string) => {
      if (command === "session_attach") return attach.promise;
      return undefined;
    });
    const invoke = bridge as unknown as AgentSessionDeps["invoke"];
    const { handle } = historyHarness(invoke);

    handle.dispose();
    attach.resolve(41);
    for (let index = 0; index < 6; index += 1) await Promise.resolve();

    expect(bridge.mock.calls.filter(([command]) => command === "session_detach")).toHaveLength(1);
    expect(bridge).toHaveBeenCalledWith("session_detach", { subscriptionId: 41 });
    expect(bridge.mock.calls.filter(([command]) => command === "session_close")).toHaveLength(0);
  });

  it("reports timeout distinctly from an artifact", async () => {
    vi.useFakeTimers();
    try {
      const invoke = vi.fn(async (command: string) =>
        command === "session_attach" ? 41 : undefined,
      ) as unknown as AgentSessionDeps["invoke"];
      const timeoutHarness = historyHarness(invoke);
      await Promise.resolve();
      vi.advanceTimersByTime(5_000);
      expect(timeoutHarness.results.at(-1)).toMatchObject({ status: "timeout" });
      expect(timeoutHarness.results.at(-1)).not.toMatchObject({ status: "artifact" });

      const artifactHarness = historyHarness(invoke);
      await Promise.resolve();
      artifactHarness.emit({
        type: "agent_message",
        messageId: "assistant-1",
        text: "```html\n<main>Reopened</main>\n```",
      });
      vi.advanceTimersByTime(DESIGN_HISTORY_OPEN_QUIET_MS);
      expect(artifactHarness.results.at(-1)).toEqual({
        status: "artifact",
        html: "<main>Reopened</main>",
      });
      artifactHarness.handle.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("uses the formatted number when choosing singular or plural timeout wording", async () => {
    vi.useFakeTimers();
    try {
      const invoke = vi.fn(async (command: string) =>
        command === "session_attach" ? 41 : undefined,
      ) as unknown as AgentSessionDeps["invoke"];
      const singularHarness = historyHarness(invoke, { timeoutMs: 999 });
      await Promise.resolve();
      vi.advanceTimersByTime(999);
      expect(singularHarness.results.at(-1)).toEqual({
        status: "timeout",
        message: "The transcript did not produce a design within 1 second.",
      });

      const pluralHarness = historyHarness(invoke, { timeoutMs: 1_500 });
      await Promise.resolve();
      vi.advanceTimersByTime(1_500);
      expect(pluralHarness.results.at(-1)).toEqual({
        status: "timeout",
        message: "The transcript did not produce a design within 1.5 seconds.",
      });
    } finally {
      vi.useRealTimers();
    }
  });

  it("settles an artifact during the quiescence window and detaches once", async () => {
    vi.useFakeTimers();
    try {
      const bridge = vi.fn(async (command: string) =>
        command === "session_attach" ? 41 : undefined,
      );
      const invoke = bridge as unknown as AgentSessionDeps["invoke"];
      const harness = historyHarness(invoke);
      await Promise.resolve();

      harness.emit({
        type: "agent_message",
        messageId: "assistant-1",
        text: "```html\n<main>Reopened</main>\n```",
      });
      expect(harness.results.at(-1)).toEqual({ status: "loading" });
      vi.advanceTimersByTime(DESIGN_HISTORY_OPEN_QUIET_MS);
      expect(harness.results.at(-1)).toEqual({
        status: "artifact",
        html: "<main>Reopened</main>",
      });

      expect(harness.results).toHaveLength(2);
      expect(bridge.mock.calls.filter(([command]) => command === "session_detach")).toHaveLength(1);

      harness.handle.dispose();
      expect(bridge.mock.calls.filter(([command]) => command === "session_detach")).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("settles on the latest artifact after replay quiescence", async () => {
    vi.useFakeTimers();
    try {
      const invoke = vi.fn(async (command: string) =>
        command === "session_attach" ? 41 : undefined,
      ) as unknown as AgentSessionDeps["invoke"];
      const harness = historyHarness(invoke);
      await Promise.resolve();

      harness.emit({
        type: "agent_message",
        messageId: "assistant-1",
        text: "```html\n<main>A</main>\n```",
      });
      vi.advanceTimersByTime(DESIGN_HISTORY_OPEN_QUIET_MS - 1);
      expect(harness.results.some((result) => result.status === "artifact")).toBe(false);

      harness.emit({
        type: "agent_message",
        messageId: "assistant-2",
        text: "```html\n<main>B</main>\n```",
      });
      vi.advanceTimersByTime(DESIGN_HISTORY_OPEN_QUIET_MS - 1);
      expect(harness.results.some((result) => result.status === "artifact")).toBe(false);

      vi.advanceTimersByTime(1);
      expect(harness.results).toContainEqual({ status: "artifact", html: "<main>B</main>" });
      expect(harness.results).not.toContainEqual({ status: "artifact", html: "<main>A</main>" });
      harness.handle.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("uses an observed artifact instead of timing out at the hard limit", async () => {
    vi.useFakeTimers();
    try {
      const invoke = vi.fn(async (command: string) =>
        command === "session_attach" ? 41 : undefined,
      ) as unknown as AgentSessionDeps["invoke"];
      const harness = historyHarness(invoke);
      await Promise.resolve();
      vi.advanceTimersByTime(4_999);
      harness.emit({
        type: "agent_message",
        messageId: "assistant-late",
        text: "```html\n<main>Late</main>\n```",
      });
      vi.advanceTimersByTime(1);

      expect(harness.results.at(-1)).toEqual({ status: "artifact", html: "<main>Late</main>" });
      harness.handle.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("reports an oversized artifact as a failure instead of waiting for timeout", async () => {
    vi.useFakeTimers();
    try {
      const invoke = vi.fn(async (command: string) =>
        command === "session_attach" ? 41 : undefined,
      ) as unknown as AgentSessionDeps["invoke"];
      const harness = historyHarness(invoke);
      await Promise.resolve();
      harness.emit({
        type: "agent_message",
        messageId: "assistant-oversized",
        text: `Generated:\n\`\`\`html\n<div>${"x".repeat(MAX_ARTIFACT_BYTES)}</div>\n\`\`\``,
      });
      vi.advanceTimersByTime(DESIGN_HISTORY_OPEN_QUIET_MS);

      expect(harness.results.at(-1)).toEqual({
        status: "failed",
        message: ARTIFACT_TOO_LARGE_MESSAGE,
      });
    } finally {
      vi.useRealTimers();
    }
  });
});
