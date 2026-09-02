// @vitest-environment happy-dom
// happy-dom does not model sandbox origin semantics. These tests handcraft
// MessageEvent origins, so passing here does not prove the real browser frame
// has a non-opaque origin or can receive replies addressed to that origin.

import { afterEach, describe, expect, it, vi } from "vitest";
import { createPluginBridge, deriveSessionProvider, sessionToPluginSession } from "./pluginBridge";
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
      capabilities: ["oracle.search"],
    });

    send(pluginWindow, PLUGIN_ORIGIN, {
      v: 1,
      id: "not-routed",
      kind: "invoke",
      method: "oracle.search",
      payload: { query: "where" },
    });

    expect(pluginWindow.postMessage).toHaveBeenCalledWith(
      {
        v: 1,
        id: "not-routed",
        kind: "error",
        message: 'The host cannot route plugin method "oracle.search" yet',
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
  });

  it("maps sessions to the privacy-safe feed without workspace or process fields", () => {
    const session: Session = {
      id: "session-1",
      workspaceId: "workspace-secret",
      kind: "terminal",
      title: "Pi shell",
      state: { type: "live", generation: 4 },
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
        state: { type: "recovered", generation: 4, truncated: true },
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
        } satisfies Session,
      ])
      .mockResolvedValueOnce([
        {
          id: "session-1",
          workspaceId: null,
          kind: "terminal",
          title: "Claude shell",
          state: { type: "ended", generation: 1, code: 0 },
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
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
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
    await vi.advanceTimersByTimeAsync(5000);
    for (let index = 0; index < 5; index += 1) await Promise.resolve();
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
        } satisfies Session,
      ])
      .mockResolvedValueOnce([
        {
          id: "session-1",
          workspaceId: null,
          kind: "terminal",
          title: "x".repeat(1024 * 1024),
          state: { type: "live", generation: 1 },
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
    for (let index = 0; index < 5; index += 1) await Promise.resolve();
    await vi.advanceTimersByTimeAsync(5000);
    expect(pluginWindow.postMessage).not.toHaveBeenCalledWith(
      expect.objectContaining({ kind: "event" }),
      PLUGIN_ORIGIN,
    );
    bridge.dispose();
  });
});

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
