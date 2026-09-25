// @vitest-environment happy-dom

import { StrictMode } from "react";
import type { ReactNode } from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RootErrorBoundary } from "./RootErrorBoundary";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("RootErrorBoundary", () => {
  let container: HTMLDivElement;
  let root: Root;
  let errorSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    errorSpy = vi.spyOn(console, "error").mockImplementation(() => undefined);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    errorSpy.mockRestore();
    const location = window.location as unknown as Record<string, unknown>;
    if (Object.hasOwn(location, "reload")) delete location.reload;
  });

  function boundaryLogs(): unknown[][] {
    return errorSpy.mock.calls.filter(
      (call: unknown[]) => call[0] === "[RootErrorBoundary] Unhandled render error",
    );
  }

  function stubDocumentReload(): ReturnType<typeof vi.fn> {
    const reload = vi.fn();
    Object.defineProperty(window.location, "reload", { configurable: true, value: reload });
    return reload;
  }

  it("shows the root fallback instead of a blank page", async () => {
    function Broken(): ReactNode {
      throw new Error("root render failed");
    }

    await act(async () => {
      root.render(
        <RootErrorBoundary>
          <Broken />
        </RootErrorBoundary>,
      );
    });

    const alert = container.querySelector('[role="alert"]');
    if (alert === null) throw new Error("root fallback did not render");
    expect(alert.textContent).toContain("Devboule ran into a problem.");
    expect(alert.textContent).toContain("restarts the app on the Workspace surface");
    expect(alert.textContent).toContain("root render failed");
    // Pinned, Paseo's compact-footer idiom: the only recovery control must
    // not scroll away at high zoom.
    expect(alert.querySelector(".root-fallback-footer .boundary-reload")?.textContent).toBe(
      "Reload",
    );
    expect(boundaryLogs()).toHaveLength(1);
  });

  it("reload performs a real document reload", async () => {
    function Broken(): ReactNode {
      throw new Error("chunk render failed");
    }
    const reload = stubDocumentReload();

    await act(async () => {
      root.render(
        <RootErrorBoundary>
          <Broken />
        </RootErrorBoundary>,
      );
    });
    expect(container.querySelector('[role="alert"]')).not.toBeNull();

    // A generation-key remount could never recover a failed lazy() import —
    // React caches the rejection — so the button reloads the document.
    const button = container.querySelector<HTMLButtonElement>(".boundary-reload");
    if (button === null) throw new Error("root reload control did not render");
    await act(async () => button.click());
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("logs once under StrictMode double mount", async () => {
    function Broken(): ReactNode {
      throw new Error("strict render failed");
    }

    await act(async () => {
      root.render(
        <StrictMode>
          <RootErrorBoundary>
            <Broken />
          </RootErrorBoundary>
        </StrictMode>,
      );
    });
    expect(container.querySelector('[role="alert"]')).not.toBeNull();
    // componentDidCatch runs once per caught error, not once per render.
    expect(boundaryLogs()).toHaveLength(1);
  });
});
