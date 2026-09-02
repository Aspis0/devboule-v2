// @vitest-environment happy-dom
// happy-dom does not model sandbox origin semantics: passing here does not
// prove the real browser frame has a non-opaque origin. These tests cover the
// surface's own contract (markup, and the frame/bridge lifetime below).

import { renderToStaticMarkup } from "react-dom/server";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/tauri")>();
  return {
    ...actual,
    invokeTyped: vi.fn().mockResolvedValue({}),
    sessionsList: vi.fn().mockResolvedValue([]),
  };
});

import { PluginSurface } from "./PluginSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("PluginSurface", () => {
  it("loads an HTML entry document in a foreign-origin iframe", () => {
    const markup = renderToStaticMarkup(
      <PluginSurface
        pluginId="polis"
        entry="ui/index.html"
        assetOrigin="http://plugin.localhost/"
        capabilities={["oracle.search"]}
      />,
    );

    expect(markup).toContain('src="http://plugin.localhost/polis/ui/index.html"');
    expect(markup).toContain('sandbox="allow-scripts allow-same-origin"');
  });

  it("renders a readable failure instead of an iframe for a missing entry", () => {
    const markup = renderToStaticMarkup(
      <PluginSurface
        pluginId="polis"
        entry={null}
        assetOrigin="http://plugin.localhost"
        capabilities={[]}
      />,
    );

    expect(markup).toContain("did not declare a UI entry path");
    expect(markup).not.toContain("<iframe");
  });
});

// The frame's document and its bridge must share a lifetime. The host deletes
// its sessions.watch subscriber when a bridge is disposed, and a plugin
// document has no way to learn that its bridge was rebuilt underneath it: if
// the capability list changes while the same document stays loaded, the feed
// dies silently with the plugin still believing it is subscribed. The iframe
// key therefore ties the document to the bridge identity, so every bridge
// rebuild is also a document reload and a fresh subscription.
describe("PluginSurface frame lifetime", () => {
  let root: Root | null = null;
  let host: HTMLElement | null = null;

  function renderSurface(capabilities: readonly string[]): void {
    if (host === null) {
      host = document.createElement("div");
      document.body.appendChild(host);
      root = createRoot(host);
    }
    act(() => {
      root?.render(
        <PluginSurface
          pluginId="polis"
          entry="index.html"
          assetOrigin="http://plugin.localhost"
          capabilities={capabilities}
        />,
      );
    });
  }

  afterEach(() => {
    act(() => {
      root?.unmount();
    });
    root = null;
    host?.remove();
    host = null;
  });

  it("reloads the plugin document when the capability list changes", () => {
    renderSurface(["workspace.root", "city.get"]);
    const before = document.querySelector("iframe");
    expect(before).not.toBeNull();

    renderSurface(["workspace.root", "city.get", "sessions.watch"]);
    const after = document.querySelector("iframe");
    expect(after).not.toBeNull();
    expect(after).not.toBe(before);
  });

  it("keeps the same document across re-renders that change nothing the bridge sees", () => {
    renderSurface(["workspace.root", "city.get"]);
    const before = document.querySelector("iframe");

    renderSurface(["workspace.root", "city.get"]);
    const after = document.querySelector("iframe");
    expect(after).toBe(before);
  });
});
