// @vitest-environment happy-dom
// happy-dom does not model sandbox origin semantics. These tests handcraft
// MessageEvent origins, so passing here does not prove the real browser frame
// has a non-opaque origin or can receive replies addressed to that origin.

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createPluginBridge,
  deriveSessionProvider,
  resetSessionWatchesForTests,
  sessionToPluginSession,
} from "./pluginBridge";
import type { Session } from "../../types/ipc";

const PLUGIN_ORIGIN = "http://plugin.localhost";

function testFrame(): { iframe: HTMLIFrameElement; pluginWindow: Window } {
  const iframe = document.createElement("iframe");
  document.body.appendChild(iframe);
  const pluginWindow = iframe.contentWindow;
  if (pluginWindow === null) throw new Error("happy-dom did not create an iframe window");
  vi.spyOn(pluginWindow, "postMessage").mockImplementation(() => undefined);
  return { iframe, pluginWindow };
}

function send(
  pluginWindow: Window,
  origin: string,
  data: unknown,
  source: Window = pluginWindow,
): void {
  window.dispatchEvent(new MessageEvent("message", { data, origin, source }));
}

afterEach(() => {
  resetSessionWatchesForTests();
  document.body.replaceChildren();
  vi.useRealTimers();
});

describe("createPluginBridge", () => {
  it("ignores a valid-looking invoke from the wrong origin", () => {
    const { iframe, pluginWindow } = testFrame();
    createPluginBridge({
      iframe,
      pluginId: "polis",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
    });

    send(pluginWindow, "http://evil.localhost", {
      v: 1,
      id: "wrong-origin",
      kind: "invoke",
      method: "oracle.search",
    });

    expect(pluginWindow.postMessage).not.toHaveBeenCalled();
  });

  it("ignores a valid invoke from the right origin but a different source window", () => {
    const { iframe, pluginWindow } = testFrame();
    const otherWindow = {} as Window;
    createPluginBridge({
      iframe,
      pluginId: "polis",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
    });

    send(
      pluginWindow,
      PLUGIN_ORIGIN,
      {
        v: 1,
        id: "wrong-source",
        kind: "invoke",
        method: "oracle.search",
      },
      otherWindow,
    );

    expect(pluginWindow.postMessage).not.toHaveBeenCalled();
  });

  it("refuses a method whose capability was not requested", () => {
    const { iframe, pluginWindow } = testFrame();
    createPluginBridge({
      iframe,
      pluginId: "polis",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: [],
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "not-requested",
      kind: "invoke",
      method: "oracle.search",
    });

    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      {
        v: 1,
        id: "not-requested",
        kind: "error",
        message: 'Plugin capability "oracle.search" was not requested in the manifest',
      },
      PLUGIN_ORIGIN,
    );
  });

  it("forwards a granted method through route and posts the result", async () => {
    const { iframe, pluginWindow } = testFrame();
    const route = vi.fn().mockResolvedValue({ root: "C:\\\\repo", status: "ok" });
    createPluginBridge({
      iframe,
      pluginId: "polis",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["workspace.root"],
      route,
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "root-1",
      kind: "invoke",
      method: "workspace.root",
    });

    expect(route).toHaveBeenCalledWith("workspace.root", undefined);
    await Promise.resolve();
    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      {
        v: 1,
        id: "root-1",
        kind: "result",
        value: { root: "C:\\\\repo", status: "ok" },
      },
      PLUGIN_ORIGIN,
    );
  });

  it("posts a route rejection as a bridge error", async () => {
    const { iframe, pluginWindow } = testFrame();
    const route = vi.fn().mockRejectedValue({
      code: "io",
      message: "the plugin backend process exited during the request",
    });
    createPluginBridge({
      iframe,
      pluginId: "polis",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["workspace.root"],
      route,
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "dead-1",
      kind: "invoke",
      method: "workspace.root",
    });

    await Promise.resolve();
    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      {
        v: 1,
        id: "dead-1",
        kind: "error",
        code: "io",
        message: "the plugin backend process exited during the request",
      },
      PLUGIN_ORIGIN,
    );
  });

  it("does not call route for a method whose capability was not requested", () => {
    const { iframe, pluginWindow } = testFrame();
    const route = vi.fn();
    createPluginBridge({
      iframe,
      pluginId: "polis",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["workspace.root"],
      route,
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "not-requested",
      kind: "invoke",
      method: "oracle.search",
    });

    expect(route).not.toHaveBeenCalled();
    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      {
        v: 1,
        id: "not-requested",
        kind: "error",
        message: 'Plugin capability "oracle.search" was not requested in the manifest',
      },
      PLUGIN_ORIGIN,
    );
  });

  it("reports that an allowed method cannot be routed yet", () => {
    const { iframe, pluginWindow } = testFrame();
    createPluginBridge({
      iframe,
      pluginId: "polis",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["future.method"],
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "not-routed",
      kind: "invoke",
      method: "future.method",
    });

    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      {
        v: 1,
        id: "not-routed",
        kind: "error",
        message: 'The host cannot route plugin method "future.method" yet',
      },
      PLUGIN_ORIGIN,
    );
  });

  it("correlates a result reply and times out a request with no reply", async () => {
    vi.useFakeTimers();
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "polis",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      timeoutMs: 10,
    });

    const result = bridge.invoke("oracle.search", { query: "where" });
    const request = vi.mocked(pluginWindow.postMessage).mock.calls[0]?.[0];
    if (!isRecord(request)) throw new Error("bridge did not send a request");
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: request.id,
      kind: "result",
      value: { answer: "here" },
    });
    await expect(result).resolves.toEqual({ answer: "here" });

    const timeout = bridge.invoke("oracle.search");
    const timeoutExpectation = expect(timeout).rejects.toThrow("timed out");
    await vi.advanceTimersByTimeAsync(10);
    await timeoutExpectation;
    bridge.dispose();
  });

  it("derives only the provisional provider vocabulary and keeps unknown titles unknown", () => {
    expect(deriveSessionProvider("Claude review")).toBe("claude");
    expect(deriveSessionProvider("Codex terminal")).toBe("codex");
    expect(deriveSessionProvider("OpenCode task")).toBe("opencode");
    expect(deriveSessionProvider("Copilot fix")).toBe("copilot");
    expect(deriveSessionProvider("ordinary terminal session")).toBeNull();
    expect(deriveSessionProvider("pipeline build")).toBeNull();
    expect(deriveSessionProvider("capital works")).toBeNull();
    expect(deriveSessionProvider("GitHub Copilot session")).toBe("copilot");
    expect(deriveSessionProvider("pi repl")).toBe("pi");
    expect(deriveSessionProvider("Claude Code")).toBe("claude");
  });

  it("maps an unverified recovered transcript to the privacy-safe finished feed", () => {
    const session: Session = {
      id: "session-1",
      workspaceId: "workspace-secret",
      kind: "terminal",
      title: "Pi shell",
      state: { type: "live", generation: 4 },
      elapsedMs: 0,
    };

    const mapped = sessionToPluginSession(session);
    expect(mapped).toEqual({
      id: "session-1",
      provider: "pi",
      state: "working",
      title: "Pi shell",
    });
    expect(JSON.stringify(mapped)).not.toContain("workspace");
    expect(JSON.stringify(mapped)).not.toContain("generation");
    expect(JSON.stringify(mapped)).not.toContain("terminal");

    expect(
      sessionToPluginSession({
        ...session,
        title: "recovered session",
        state: {
          type: "recovered",
          generation: 4,
          integrity: { kind: "unverifiable", droppedFrames: 4, droppedBytes: 12 * 1024 },
        },
      }),
    ).toEqual({
      id: "session-1",
      provider: null,
      state: "finished",
      title: "recovered session [recovered transcript]",
    });
  });

  it("does not serve a requested sessions.watch capability unless the host allowlist says so", () => {
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "polis",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["sessions.watch"],
      servedCapabilities: [],
      sessionList: vi.fn(),
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "watch-denied",
      kind: "invoke",
      method: "sessions.watch",
    });

    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        v: 1,
        id: "watch-denied",
        kind: "error",
        code: "capability_not_supported",
      }),
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });

  it("pushes session updates to the current bridge and stops them after the last bridge is disposed", async () => {
    vi.useFakeTimers();
    const first = testFrame();
    const sessions = vi
      .fn()
      .mockResolvedValueOnce([
        {
          id: "session-1",
          workspaceId: null,
          kind: "terminal",
          title: "Claude shell",
          state: { type: "live", generation: 1 },
          elapsedMs: 0,
        } satisfies Session,
      ])
      .mockResolvedValueOnce([
        {
          id: "session-1",
          workspaceId: null,
          kind: "terminal",
          title: "Claude shell",
          state: { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } },
          elapsedMs: null,
        } satisfies Session,
      ]);
    const firstBridge = createPluginBridge({
      iframe: first.iframe,
      pluginId: "polis",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["sessions.watch"],
      sessionList: sessions,
    });

    send(first.pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "watch-1",
      kind: "invoke",
      method: "sessions.watch",
      payload: { subscriptionId: "watch-sub-1" },
    });
    await flushPromises();
    expect(first.pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "watch-1",
        kind: "result",
        value: {
          sessions: [
            { id: "session-1", provider: "claude", state: "working", title: "Claude shell" },
          ],
        },
      }),
      PLUGIN_ORIGIN,
    );

    const second = testFrame();
    firstBridge.dispose();
    const secondBridge = createPluginBridge({
      iframe: second.iframe,
      pluginId: "polis",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["sessions.watch"],
      sessionList: sessions,
    });
    send(second.pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "watch-2",
      kind: "invoke",
      method: "sessions.watch",
      payload: { subscriptionId: "watch-sub-1" },
    });
    await flushPromises();
    await vi.advanceTimersByTimeAsync(5000);
    await flushPromises();
    expect(sessions).toHaveBeenCalledTimes(2);
    expect(first.pluginWindow.postMessage).not.toHaveBeenCalledWith(
      expect.objectContaining({ kind: "event" }),
      PLUGIN_ORIGIN,
    );
    expect(second.pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "watch-sub-1",
        kind: "event",
        event: "sessions.update",
        value: {
          sessions: [
            { id: "session-1", provider: "claude", state: "finished", title: "Claude shell" },
          ],
        },
      }),
      PLUGIN_ORIGIN,
    );
    secondBridge.dispose();
    await vi.runOnlyPendingTimersAsync();
    const calls = sessions.mock.calls.length;
    await vi.advanceTimersByTimeAsync(5000);
    expect(sessions).toHaveBeenCalledTimes(calls);
  });

  it("does not restart polling during a bridge rebind before a new snapshot is delivered", async () => {
    vi.useFakeTimers();
    const first = testFrame();
    const sessions = vi.fn().mockResolvedValue([
      {
        id: "session-1",
        workspaceId: null,
        kind: "terminal",
        title: "Claude shell",
        state: { type: "live", generation: 1 },
        elapsedMs: 0,
      } satisfies Session,
    ]);
    const firstBridge = createPluginBridge({
      iframe: first.iframe,
      pluginId: "polis-rebind-poll",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["sessions.watch"],
      sessionList: sessions,
    });
    send(first.pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "rebind-watch-1",
      kind: "invoke",
      method: "sessions.watch",
      payload: { subscriptionId: "rebind-sub-1" },
    });
    await flushPromises();
    firstBridge.dispose();

    const second = testFrame();
    const secondBridge = createPluginBridge({
      iframe: second.iframe,
      pluginId: "polis-rebind-poll",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["sessions.watch"],
      sessionList: sessions,
    });
    await vi.advanceTimersByTimeAsync(5000);
    expect(sessions).toHaveBeenCalledTimes(1);
    secondBridge.dispose();
  });

  it("does not emit a session event whose value exceeds the response cap", async () => {
    vi.useFakeTimers();
    const { iframe, pluginWindow } = testFrame();
    const sessions = vi
      .fn()
      .mockResolvedValueOnce([
        {
          id: "session-1",
          workspaceId: null,
          kind: "terminal",
          title: "small",
          state: { type: "live", generation: 1 },
          elapsedMs: 0,
        } satisfies Session,
      ])
      .mockResolvedValueOnce([
        {
          id: "session-1",
          workspaceId: null,
          kind: "terminal",
          title: "x".repeat(1024 * 1024),
          state: { type: "live", generation: 1 },
          elapsedMs: 0,
        } satisfies Session,
      ])
      .mockResolvedValueOnce([
        {
          id: "session-1",
          workspaceId: null,
          kind: "terminal",
          title: "small-recovered",
          state: { type: "live", generation: 1 },
          elapsedMs: 0,
        } satisfies Session,
      ]);
    const bridge = createPluginBridge({
      iframe,
      pluginId: "polis-cap",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["sessions.watch"],
      sessionList: sessions,
    });
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "watch-cap",
      kind: "invoke",
      method: "sessions.watch",
      payload: { subscriptionId: "cap-sub" },
    });
    await flushPromises();
    await vi.advanceTimersByTimeAsync(5000);
    await flushPromises();
    expect(pluginWindow.postMessage).not.toHaveBeenCalledWith(
      expect.objectContaining({ kind: "event" }),
      PLUGIN_ORIGIN,
    );
    await vi.advanceTimersByTimeAsync(5000);
    await flushPromises();
    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "cap-sub",
        kind: "event",
        value: { sessions: [expect.objectContaining({ title: "small-recovered" })] },
      }),
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });

  it("unwatches a session feed and clears the host poll when the last subscriber closes", async () => {
    vi.useFakeTimers();
    const { iframe, pluginWindow } = testFrame();
    const sessions = vi.fn().mockResolvedValue([
      {
        id: "session-1",
        workspaceId: null,
        kind: "terminal",
        title: "Claude shell",
        state: { type: "live", generation: 1 },
        elapsedMs: 0,
      } satisfies Session,
    ]);
    const bridge = createPluginBridge({
      iframe,
      pluginId: "polis-unwatch",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["sessions.watch"],
      sessionList: sessions,
    });
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "watch-subscribe",
      kind: "invoke",
      method: "sessions.watch",
      payload: { subscriptionId: "unwatch-1", action: "subscribe" },
    });
    await flushPromises();
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "watch-unsubscribe",
      kind: "invoke",
      method: "sessions.watch",
      payload: { subscriptionId: "unwatch-1", action: "unsubscribe" },
    });
    await flushPromises();
    const callsAfterUnwatch = sessions.mock.calls.length;
    await vi.advanceTimersByTimeAsync(5000);
    expect(sessions).toHaveBeenCalledTimes(callsAfterUnwatch);
    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ id: "watch-unsubscribe", kind: "result" }),
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });

  it("does not leak a resolved initial promise across dispose, rebind, and resubscribe", async () => {
    vi.useFakeTimers();
    const first = testFrame();
    let resolveFirst: ((value: Session[]) => void) | undefined;
    const sessions = vi
      .fn()
      .mockImplementationOnce(
        () =>
          new Promise<Session[]>((resolve) => {
            resolveFirst = resolve;
          }),
      )
      .mockResolvedValueOnce([
        {
          id: "session-1",
          workspaceId: null,
          kind: "terminal",
        title: "Claude shell",
        state: { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } },
        elapsedMs: null,
        } satisfies Session,
      ]);
    const firstBridge = createPluginBridge({
      iframe: first.iframe,
      pluginId: "polis-interleave",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["sessions.watch"],
      sessionList: sessions,
    });
    send(first.pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "first-watch",
      kind: "invoke",
      method: "sessions.watch",
      payload: { subscriptionId: "interleave-1", action: "subscribe" },
    });
    firstBridge.dispose();
    resolveFirst?.([
      {
        id: "session-1",
        workspaceId: null,
        kind: "terminal",
          title: "Claude shell",
          state: { type: "live", generation: 1 },
          elapsedMs: 0,
      },
    ]);
    await Promise.resolve();

    const second = testFrame();
    const secondBridge = createPluginBridge({
      iframe: second.iframe,
      pluginId: "polis-interleave",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["sessions.watch"],
      sessionList: sessions,
    });
    send(second.pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "second-watch",
      kind: "invoke",
      method: "sessions.watch",
      payload: { subscriptionId: "interleave-2", action: "subscribe" },
    });
    await flushPromises();
    expect(sessions).toHaveBeenCalledTimes(2);
    expect(second.pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: "result",
        id: "second-watch",
        value: {
          sessions: [expect.objectContaining({ state: "finished" })],
        },
      }),
      PLUGIN_ORIGIN,
    );
    secondBridge.dispose();
  });

  it("times out a hung initial or refresh source without wedging the watch", async () => {
    vi.useFakeTimers();
    const { iframe, pluginWindow } = testFrame();
    const sessions = vi.fn().mockImplementationOnce(() => new Promise<Session[]>(() => {}));
    const bridge = createPluginBridge({
      iframe,
      pluginId: "polis-timeout",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["sessions.watch"],
      sessionList: sessions,
    });
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "timeout-watch",
      kind: "invoke",
      method: "sessions.watch",
      payload: { subscriptionId: "timeout-1", action: "subscribe" },
    });
    await vi.advanceTimersByTimeAsync(10_000);
    await flushPromises();
    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ id: "timeout-watch", kind: "error", code: "timeout" }),
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });

  it("resets a hung refresh and retries on the next interval", async () => {
    vi.useFakeTimers();
    const { iframe, pluginWindow } = testFrame();
    const sessions = vi
      .fn()
      .mockResolvedValueOnce([
        {
          id: "session-1",
          workspaceId: null,
          kind: "terminal",
          title: "initial",
          state: { type: "live", generation: 1 },
          elapsedMs: 0,
        } satisfies Session,
      ])
      .mockImplementationOnce(() => new Promise<Session[]>(() => {}))
      .mockResolvedValueOnce([
        {
          id: "session-1",
          workspaceId: null,
          kind: "terminal",
          title: "after-timeout",
          state: { type: "live", generation: 1 },
          elapsedMs: 0,
        } satisfies Session,
      ]);
    const bridge = createPluginBridge({
      iframe,
      pluginId: "polis-refresh-timeout",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["sessions.watch"],
      sessionList: sessions,
    });
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "refresh-timeout-watch",
      kind: "invoke",
      method: "sessions.watch",
      payload: { subscriptionId: "refresh-timeout-1", action: "subscribe" },
    });
    await flushPromises();
    await vi.advanceTimersByTimeAsync(5000);
    await flushPromises();
    await vi.advanceTimersByTimeAsync(10_000);
    await flushPromises();
    await vi.advanceTimersByTimeAsync(5000);
    await flushPromises();
    expect(sessions).toHaveBeenCalledTimes(3);
    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "refresh-timeout-1",
        kind: "event",
        value: { sessions: [expect.objectContaining({ title: "after-timeout" })] },
      }),
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });

  it("does not send raw session-watch source errors into the iframe", async () => {
    const { iframe, pluginWindow } = testFrame();
    const sessions = vi.fn().mockRejectedValue(new Error("C:\\Users\\secret\\workspace"));
    const bridge = createPluginBridge({
      iframe,
      pluginId: "polis-private-error",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["sessions.watch"],
      sessionList: sessions,
    });
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "private-watch",
      kind: "invoke",
      method: "sessions.watch",
      payload: { subscriptionId: "private-1", action: "subscribe" },
    });
    await flushPromises();
    const errorReply = vi
      .mocked(pluginWindow.postMessage)
      .mock.calls.map(([message]) => message)
      .find((message) => isRecord(message) && message.kind === "error");
    expect(errorReply).toEqual(
      expect.objectContaining({ id: "private-watch", message: "sessions.watch failed" }),
    );
    expect(JSON.stringify(errorReply)).not.toContain("workspace");
    bridge.dispose();
  });

  it("refuses oracle.search when the host allowlist does not serve it", async () => {
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-denied",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      servedCapabilities: [],
      oracleSearch: vi.fn(),
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-denied-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "where" },
    });
    await flushPromises();

    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      {
        v: 1,
        id: "oracle-denied-request",
        kind: "error",
        code: "capability_not_supported",
        message: 'The host does not serve plugin capability "oracle.search"',
      },
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });

  it("rejects oracle.search when the query is absent", async () => {
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-missing-query",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch: vi.fn(),
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-missing-query-request",
      kind: "invoke",
      method: "oracle.search",
    });
    await flushPromises();

    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "oracle-missing-query-request",
        kind: "error",
        code: "invalid_request",
        message: "oracle.search requires a non-empty query string",
      }),
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });

  it("rejects oracle.search when the query is an empty string", async () => {
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-empty-query",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch: vi.fn(),
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-empty-query-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "" },
    });
    await flushPromises();

    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "oracle-empty-query-request",
        kind: "error",
        code: "invalid_request",
        message: "oracle.search requires a non-empty query string",
      }),
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });

  it("rejects oracle.search when the query is whitespace-only", async () => {
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-whitespace-query",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch: vi.fn(),
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-whitespace-query-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "  \t\n" },
    });
    await flushPromises();

    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "oracle-whitespace-query-request",
        kind: "error",
        code: "invalid_request",
        message: "oracle.search requires a non-empty query string",
      }),
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });

  it("rejects oracle.search when query is not a string", async () => {
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-non-string-query",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch: vi.fn(),
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-non-string-query-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: 42 },
    });
    await flushPromises();

    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "oracle-non-string-query-request",
        kind: "error",
        code: "invalid_request",
        message: "oracle.search requires a non-empty query string",
      }),
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });

  it("rejects oracle.search when the trimmed query exceeds 4096 characters", async () => {
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-long-query",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch: vi.fn(),
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-long-query-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: ` ${"q".repeat(4097)} ` },
    });
    await flushPromises();

    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "oracle-long-query-request",
        kind: "error",
        code: "invalid_request",
        message: "oracle.search query is too long (maximum 4096 characters)",
      }),
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });

  it("counts astral Oracle query characters as code points", async () => {
    const query = "🧭".repeat(4096);
    const { iframe, pluginWindow } = testFrame();
    const oracleSearch = vi.fn().mockResolvedValue({
      query,
      results: [{ path: "unicode.ts", line_start: 1, line_end: 1, snippet: "", score: 1 }],
    });
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-code-points",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch,
    });
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-code-points-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query },
    });
    await flushPromises();

    expect(oracleSearch).toHaveBeenCalledWith(query);
    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ id: "oracle-code-points-request", kind: "result" }),
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });

  it("projects Oracle results without content or scores, preserves order, and caps at ten", async () => {
    const { iframe, pluginWindow } = testFrame();
    const results = [
      {
        path: "first.ts",
        line_start: 1,
        line_end: 4,
        focus_line_start: 2,
        focus_line_end: 3,
        snippet: "secret source text",
        score: 0.99,
        symbol_name: "firstSymbol",
        match_type: "dense" as const,
      },
      {
        path: "second.ts",
        line_start: 0,
        line_end: 0,
        snippet: "private prose",
        score: 0.5,
      },
      ...Array.from({ length: 9 }, (_, index) => ({
        path: `tail-${index}.ts`,
        line_start: index + 1,
        line_end: index + 1,
        snippet: "not for the frame",
        score: index,
      })),
    ];
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-projection",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch: vi.fn().mockResolvedValue({ query: "symbols", results }),
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-projection-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "  symbols  " },
    });
    await flushPromises();

    const reply = vi.mocked(pluginWindow.postMessage).mock.calls[0]?.[0];
    if (!isRecord(reply) || !isRecord(reply.value)) throw new Error("missing Oracle reply");
    if (!Array.isArray(reply.value.results)) throw new Error("missing projected results");
    const projected = reply.value.results;
    expect(projected).toHaveLength(10);
    expect(projected).toEqual([
      {
        path: "first.ts",
        startLine: 1,
        endLine: 4,
        focusStartLine: 2,
        focusEndLine: 3,
        symbol: "firstSymbol",
        match: "dense",
      },
      { path: "second.ts", startLine: 0, endLine: 0 },
      ...Array.from({ length: 8 }, (_, index) => ({
        path: `tail-${index}.ts`,
        startLine: index + 1,
        endLine: index + 1,
      })),
    ]);
    expect(reply.value.query).toBe("symbols");
    expect(projected[0]).not.toHaveProperty("snippet");
    expect(projected[0]).not.toHaveProperty("score");
    expect(projected[1]).not.toHaveProperty("focusStartLine");
    expect(projected[1]).not.toHaveProperty("focusEndLine");
    bridge.dispose();
  });

  it("keeps Oracle startLine zero as the legal unknown-line value", async () => {
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-zero-line",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch: vi.fn().mockResolvedValue({
        query: "prose",
        results: [
          { path: "README.md", line_start: 0, line_end: 0, snippet: "private", score: 1 },
        ],
      }),
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-zero-line-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "prose" },
    });
    await flushPromises();

    const reply = vi.mocked(pluginWindow.postMessage).mock.calls[0]?.[0];
    if (!isRecord(reply) || !isRecord(reply.value)) throw new Error("missing Oracle reply");
    expect(reply.value.results).toEqual([{ path: "README.md", startLine: 0, endLine: 0 }]);
    bridge.dispose();
  });

  it("adds index only for empty results and ignores unusable status", async () => {
    const first = testFrame();
    const firstBridge = createPluginBridge({
      iframe: first.iframe,
      pluginId: "oracle-empty-state",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch: vi.fn().mockResolvedValue({ query: "none", results: [] }),
      oracleIndexState: vi.fn().mockResolvedValue({ state: "ready", indexed_files: 4 }),
    });
    send(first.pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-empty-state-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "none" },
    });
    await flushPromises();
    const firstReply = vi.mocked(first.pluginWindow.postMessage).mock.calls[0]?.[0];
    if (!isRecord(firstReply) || !isRecord(firstReply.value)) throw new Error("missing Oracle reply");
    expect(firstReply.value).toEqual({
      query: "none",
      results: [],
      index: { state: "ready", indexedFiles: 4 },
    });
    firstBridge.dispose();

    const second = testFrame();
    const secondBridge = createPluginBridge({
      iframe: second.iframe,
      pluginId: "oracle-empty-status-failure",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch: vi.fn().mockResolvedValue({ query: "none", results: [] }),
      oracleIndexState: vi.fn().mockRejectedValue(new Error("private workspace path")),
    });
    send(second.pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-empty-status-failure-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "none" },
    });
    await flushPromises();
    const secondReply = vi.mocked(second.pluginWindow.postMessage).mock.calls[0]?.[0];
    if (!isRecord(secondReply) || !isRecord(secondReply.value)) throw new Error("missing Oracle reply");
    expect(secondReply.value).toEqual({ query: "none", results: [] });
    secondBridge.dispose();

    const third = testFrame();
    const thirdBridge = createPluginBridge({
      iframe: third.iframe,
      pluginId: "oracle-empty-status-malformed",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch: vi.fn().mockResolvedValue({ query: "none", results: [] }),
      oracleIndexState: vi.fn().mockResolvedValue({ state: "ready" }),
    });
    send(third.pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-empty-status-malformed-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "none" },
    });
    await flushPromises();
    const thirdReply = vi.mocked(third.pluginWindow.postMessage).mock.calls[0]?.[0];
    if (!isRecord(thirdReply) || !isRecord(thirdReply.value)) throw new Error("missing Oracle reply");
    expect(thirdReply.value).toEqual({ query: "none", results: [] });
    thirdBridge.dispose();
  });

  it("coalesces concurrent oracle.search calls to the latest query", async () => {
    let resolveFirst!: (value: unknown) => void;
    const oracleSearch = vi
      .fn()
      .mockReturnValueOnce(new Promise((resolve) => (resolveFirst = resolve)))
      .mockResolvedValueOnce({ query: "third", results: [] });
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-single-flight",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch,
      oracleIndexState: vi.fn().mockResolvedValue({ state: "idle" }),
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-first-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "first" },
    });
    await flushPromises();
    expect(oracleSearch).toHaveBeenCalledTimes(1);
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-second-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "second" },
    });
    await flushPromises();
    expect(pluginWindow.postMessage).not.toHaveBeenCalledWith(
      expect.objectContaining({ id: "oracle-second-request" }),
      PLUGIN_ORIGIN,
    );

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-third-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "third" },
    });
    await flushPromises();
    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "oracle-second-request",
        kind: "error",
        code: "busy",
        message: "oracle.search is already running for this plugin",
      }),
      PLUGIN_ORIGIN,
    );

    resolveFirst({ query: "first", results: [] });
    await flushPromises();
    await flushPromises();
    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ id: "oracle-third-request", kind: "result" }),
      PLUGIN_ORIGIN,
    );
    expect(oracleSearch).toHaveBeenCalledTimes(2);
    bridge.dispose();
  });

  it("does not inherit an in-flight search across a bridge rebuild", async () => {
    let resolveFirst!: (value: unknown) => void;
    const firstSearch = vi.fn().mockReturnValue(
      new Promise((resolve) => {
        resolveFirst = resolve;
      }),
    );
    const first = testFrame();
    const firstBridge = createPluginBridge({
      iframe: first.iframe,
      pluginId: "oracle-reload",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch: firstSearch,
    });
    send(first.pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-reload-first",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "first" },
    });
    await flushPromises();
    firstBridge.dispose();

    const second = testFrame();
    const secondSearch = vi.fn().mockResolvedValue({
      query: "second",
      results: [{ path: "second.ts", line_start: 1, line_end: 1, snippet: "", score: 1 }],
    });
    const secondBridge = createPluginBridge({
      iframe: second.iframe,
      pluginId: "oracle-reload",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch: secondSearch,
    });
    send(second.pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-reload-second",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "second" },
    });
    await flushPromises();

    expect(secondSearch).toHaveBeenCalledWith("second");
    expect(second.pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ id: "oracle-reload-second", kind: "result" }),
      PLUGIN_ORIGIN,
    );
    resolveFirst({ query: "first", results: [] });
    secondBridge.dispose();
  });

  it("answers a pending oracle.search as busy when the bridge is disposed", async () => {
    let resolveFirst!: (value: unknown) => void;
    const oracleSearch = vi.fn().mockReturnValueOnce(
      new Promise((resolve) => {
        resolveFirst = resolve;
      }),
    );
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-dispose-pending",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch,
    });
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-dispose-first",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "first" },
    });
    await flushPromises();
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-dispose-pending-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "pending" },
    });
    await flushPromises();

    bridge.dispose();
    resolveFirst({
      query: "first",
      results: [{ path: "first.ts", line_start: 1, line_end: 1, snippet: "", score: 1 }],
    });
    await flushPromises();

    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "oracle-dispose-pending-request",
        kind: "error",
        code: "busy",
        message: "oracle.search is already running for this plugin",
      }),
      PLUGIN_ORIGIN,
    );
  });

  it("refuses malformed oracle.search immediately without consuming a pending query", async () => {
    let resolveFirst!: (value: unknown) => void;
    const oracleSearch = vi
      .fn()
      .mockReturnValueOnce(new Promise((resolve) => (resolveFirst = resolve)))
      .mockResolvedValueOnce({
        query: "pending",
        results: [{ path: "pending.ts", line_start: 1, line_end: 1, snippet: "", score: 1 }],
      });
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-malformed-in-flight",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch,
    });
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-malformed-first",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "first" },
    });
    await flushPromises();
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-malformed-pending",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "pending" },
    });
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-malformed-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: 42 },
    });
    await flushPromises();

    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "oracle-malformed-request",
        kind: "error",
        code: "invalid_request",
      }),
      PLUGIN_ORIGIN,
    );
    expect(oracleSearch).toHaveBeenCalledTimes(1);
    resolveFirst({ query: "first", results: [{ path: "first.ts", line_start: 1, line_end: 1, snippet: "", score: 1 }] });
    await flushPromises();
    expect(oracleSearch).toHaveBeenCalledWith("pending");
    bridge.dispose();
  });

  it("does not start a queued oracle.search after the six-second response margin", async () => {
    vi.useFakeTimers();
    let resolveFirst!: (value: unknown) => void;
    const oracleSearch = vi.fn().mockReturnValueOnce(
      new Promise((resolve) => {
        resolveFirst = resolve;
      }),
    );
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-queue-budget",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch,
    });
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-queue-first",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "first" },
    });
    await flushPromises();
    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-queue-pending",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "pending" },
    });
    await flushPromises();
    await vi.advanceTimersByTimeAsync(6001);
    resolveFirst({
      query: "first",
      results: [{ path: "first.ts", line_start: 1, line_end: 1, snippet: "", score: 1 }],
    });
    await flushPromises();

    expect(oracleSearch).toHaveBeenCalledTimes(1);
    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "oracle-queue-pending",
        kind: "error",
        code: "busy",
        message: "oracle.search is already running for this plugin",
      }),
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });

  it("rejects impossible Oracle line and focus ranges from the source", async () => {
    const { iframe, pluginWindow } = testFrame();
    const oracleSearch = vi.fn();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-line-invariants",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch,
    });
    const invalidResults = [
      { path: "zero-to-five.ts", line_start: 0, line_end: 5, snippet: "", score: 1 },
      {
        path: "focus-outside.ts",
        line_start: 1,
        line_end: 2,
        focus_line_start: 3,
        focus_line_end: 4,
        snippet: "",
        score: 1,
      },
      {
        path: "focus-without-lines.ts",
        line_start: 0,
        line_end: 0,
        focus_line_start: 0,
        focus_line_end: 0,
        snippet: "",
        score: 1,
      },
    ];
    for (const [index, result] of invalidResults.entries()) {
      oracleSearch.mockResolvedValueOnce({ query: "invalid", results: [result] });
      const id = `oracle-line-invalid-${index}`;
      send(pluginWindow, PLUGIN_ORIGIN, {
        v: 1,
        id,
        kind: "invoke",
        method: "oracle.search",
        payload: { query: "invalid" },
      });
      await flushPromises();
      expect(pluginWindow.postMessage).toHaveBeenCalledWith(
        expect.objectContaining({ id, kind: "error", code: "invalid_response" }),
        PLUGIN_ORIGIN,
      );
    }
    bridge.dispose();
  });

  it("times out a hung oracle.search source after twenty seconds", async () => {
    vi.useFakeTimers();
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-timeout",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch: vi.fn().mockImplementation(() => new Promise(() => {})),
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-timeout-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "slow" },
    });
    await vi.advanceTimersByTimeAsync(20_000);
    await flushPromises();

    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      {
        v: 1,
        id: "oracle-timeout-request",
        kind: "error",
        code: "timeout",
        message: "oracle.search failed",
      },
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });

  it("does not forward Oracle source errors containing workspace paths", async () => {
    const { iframe, pluginWindow } = testFrame();
    const bridge = createPluginBridge({
      iframe,
      pluginId: "oracle-private-error",
      pluginOrigin: PLUGIN_ORIGIN,
      capabilities: ["oracle.search"],
      oracleSearch: vi.fn().mockRejectedValue(new Error("C:\\Users\\secret\\workspace")),
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "oracle-private-error-request",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "private" },
    });
    await flushPromises();

    const errorReply = vi
      .mocked(pluginWindow.postMessage)
      .mock.calls.map(([message]) => message)
      .find((message) => isRecord(message) && message.kind === "error");
    expect(errorReply).toEqual(
      expect.objectContaining({
        id: "oracle-private-error-request",
        message: "oracle.search failed",
      }),
    );
    expect(JSON.stringify(errorReply)).not.toContain("secret");
    bridge.dispose();
  });
});

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

async function flushPromises(): Promise<void> {
  for (let index = 0; index < 12; index += 1) await Promise.resolve();
}
