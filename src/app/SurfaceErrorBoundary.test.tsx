// @vitest-environment happy-dom

import { act, lazy, Suspense, useEffect } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SurfaceErrorBoundary } from "./SurfaceErrorBoundary";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("SurfaceErrorBoundary", () => {
  let container: HTMLDivElement;
  let root: Root;
  let warnSpy: ReturnType<typeof vi.spyOn>;
  let errorSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    // React logs the caught throw itself; silence both channels so the
    // runner output shows failures, not the deliberate throws.
    warnSpy = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    errorSpy = vi.spyOn(console, "error").mockImplementation(() => undefined);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    warnSpy.mockRestore();
    errorSpy.mockRestore();
    const location = window.location as unknown as Record<string, unknown>;
    if (Object.hasOwn(location, "reload")) delete location.reload;
  });

  it("shows the surface fallback and leaves the rest of the tree mounted", async () => {
    function Broken(): ReactNode {
      throw new Error("surface render failed");
    }

    await act(async () => {
      root.render(
        <>
          <aside className="shell-stand-in">sidebar</aside>
          <SurfaceErrorBoundary surfaceLabel="Workspace">
            <Broken />
          </SurfaceErrorBoundary>
        </>,
      );
    });

    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("Workspace");
    expect(alert?.textContent).toContain("surface render failed");
    expect(alert?.querySelector(".boundary-retry")?.textContent).toBe("Retry");
    // The shell around the broken surface keeps working.
    expect(container.querySelector(".shell-stand-in")?.textContent).toBe("sidebar");
    expect(warnSpy).toHaveBeenCalledWith(
      "[SurfaceErrorBoundary] Surface render failed",
      "Workspace",
      expect.any(Error),
      expect.anything(),
    );
  });

  it("retry remounts the surface: mount and cleanup counts prove it", async () => {
    let mounts = 0;
    let cleanups = 0;
    let broken = false;
    function Child(): ReactNode {
      useEffect(() => {
        mounts += 1;
        return () => {
          cleanups += 1;
        };
      }, []);
      if (broken) throw new Error("flaky surface failed");
      return <p>healthy surface child</p>;
    }
    function Harness(): ReactNode {
      return (
        <SurfaceErrorBoundary surfaceLabel="Polis">
          <Child />
        </SurfaceErrorBoundary>
      );
    }

    await act(async () => {
      root.render(<Harness />);
    });
    expect(mounts).toBe(1);

    // The update throws: React unmounts the failed subtree as it catches —
    // Paseo asserts the same cleanup count
    // (surface-error-boundary.test.tsx:68-69) — so the retry below cannot be
    // a re-render of the old instance.
    broken = true;
    await act(async () => {
      root.render(<Harness />);
    });
    expect(container.querySelector('[role="alert"]')).not.toBeNull();
    expect(cleanups).toBe(1);

    // Retry clears the error and the child mounts fresh; a re-render of the
    // old instance would not re-run its mount effect.
    broken = false;
    const retry = container.querySelector<HTMLButtonElement>(".boundary-retry");
    if (retry === null) throw new Error("surface retry control did not render");
    await act(async () => retry.click());

    expect(container.textContent).toContain("healthy surface child");
    expect(mounts).toBe(2);
  });

  it("a rejected lazy import shows the fallback with a working document reload", async () => {
    // A failed import is cached by React forever: Retry re-renders into the
    // same cached rejection, so the fallback must offer a document reload.
    // (Mirrored structure: Suspense outside the boundary, as in App.tsx.)
    const reload = vi.fn();
    Object.defineProperty(window.location, "reload", { configurable: true, value: reload });
    const RejectedSurface = lazy(() => Promise.reject(new Error("chunk load failed")));

    await act(async () => {
      root.render(
        <Suspense fallback={<div>loading surface</div>}>
          <SurfaceErrorBoundary surfaceLabel="Settings">
            <RejectedSurface />
          </SurfaceErrorBoundary>
        </Suspense>,
      );
    });
    await vi.waitFor(() => expect(container.querySelector(".surface-fallback")).not.toBeNull(), {
      timeout: 5000,
    });
    expect(container.textContent).toContain("chunk load failed");

    const reloadButton = container.querySelector<HTMLButtonElement>(".surface-fallback-reload");
    if (reloadButton === null) throw new Error("surface reload control did not render");
    await act(async () => reloadButton.click());
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it("a key change resets the boundary through a remount", async () => {
    function Broken(): ReactNode {
      throw new Error("old surface failed");
    }

    await act(async () => {
      root.render(
        <SurfaceErrorBoundary key="workspace" surfaceLabel="Workspace">
          <Broken />
        </SurfaceErrorBoundary>,
      );
    });
    expect(container.querySelector('[role="alert"]')).not.toBeNull();

    await act(async () => {
      root.render(
        <SurfaceErrorBoundary key="settings" surfaceLabel="Settings">
          <p>healthy settings surface</p>
        </SurfaceErrorBoundary>,
      );
    });
    expect(container.textContent).toContain("healthy settings surface");
  });
});
