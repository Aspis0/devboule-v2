// @vitest-environment happy-dom

import { act } from "react";
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
  });

  it("shows the surface fallback and leaves the rest of the tree mounted", async () => {
    function Broken(): ReactNode {
      throw new Error("surface render failed");
    }

    await act(async () => {
      root.render(
        <>
          <aside className="shell-stand-in">sidebar</aside>
          <SurfaceErrorBoundary surfaceLabel="Workspace" resetKey="workspace">
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

  it("retry remounts the surface so a throw-once child recovers", async () => {
    let broken = true;
    function Flaky() {
      if (broken) throw new Error("first render failed");
      return <p>healthy surface child</p>;
    }

    await act(async () => {
      root.render(
        <SurfaceErrorBoundary surfaceLabel="Polis" resetKey="polis">
          <Flaky />
        </SurfaceErrorBoundary>,
      );
    });
    expect(container.querySelector('[role="alert"]')).not.toBeNull();

    broken = false;
    const retry = container.querySelector<HTMLButtonElement>(".boundary-retry");
    if (retry === null) throw new Error("surface retry control did not render");
    await act(async () => retry.click());

    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(container.textContent).toContain("healthy surface child");
  });

  it("clears the error when the reset key changes", async () => {
    let broken = true;
    function Child() {
      if (broken) throw new Error("stale panel failed");
      return <p>recovered panel</p>;
    }
    function Harness({ resetKey }: { resetKey: string }) {
      return (
        <SurfaceErrorBoundary surfaceLabel="Changes" resetKey={resetKey}>
          <Child />
        </SurfaceErrorBoundary>
      );
    }

    await act(async () => {
      root.render(<Harness resetKey="changes" />);
    });
    expect(container.querySelector('[role="alert"]')).not.toBeNull();

    // Navigating away and back mounts the corrected panel under a new key.
    broken = false;
    await act(async () => {
      root.render(<Harness resetKey="files" />);
    });
    expect(container.textContent).toContain("recovered panel");
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
