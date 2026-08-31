// @vitest-environment happy-dom
// happy-dom does not model sandbox origin semantics. These tests handcraft
// MessageEvent origins, so passing here does not prove the real browser frame
// has a non-opaque origin or can receive replies addressed to that origin.

import { afterEach, describe, expect, it, vi } from "vitest";
import { createPluginBridge } from "./pluginBridge";

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
});

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
