// @vitest-environment happy-dom

import { StrictMode, useState, type ReactNode } from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RootErrorBoundary } from "./RootErrorBoundary";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// The App.tsx wiring in miniature: Reload is owned by the caller, and the
// generation key remounts the boundary together with the tree below it.
function ReloadHarness({ children }: { children: ReactNode }) {
  const [generation, setGeneration] = useState(0);
  return (
    <RootErrorBoundary key={generation} onReload={() => setGeneration((value) => value + 1)}>
      {children}
    </RootErrorBoundary>
  );
}

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
  });

  function boundaryLogs(): unknown[][] {
    return errorSpy.mock.calls.filter(
      (call: unknown[]) => call[0] === "[RootErrorBoundary] Unhandled render error",
    );
  }

  it("shows the root fallback instead of a blank page", async () => {
    function Broken(): ReactNode {
      throw new Error("root render failed");
    }

    await act(async () => {
      root.render(
        <RootErrorBoundary onReload={() => undefined}>
          <Broken />
        </RootErrorBoundary>,
      );
    });

    const alert = container.querySelector('[role="alert"]');
    if (alert === null) throw new Error("root fallback did not render");
    expect(alert.textContent).toContain("Devboule ran into a problem.");
    expect(alert.textContent).toContain("Reload the app to try again.");
    expect(alert.textContent).toContain("root render failed");
    expect(alert.querySelector(".boundary-reload")?.textContent).toBe("Reload");
    expect(boundaryLogs()).toHaveLength(1);
  });

  it("reload remounts the tree so a throw-once child recovers", async () => {
    let broken = true;
    function Flaky() {
      if (broken) throw new Error("first app render failed");
      return <main>healthy app tree</main>;
    }

    await act(async () => {
      root.render(
        <ReloadHarness>
          <Flaky />
        </ReloadHarness>,
      );
    });
    expect(container.querySelector('[role="alert"]')).not.toBeNull();

    broken = false;
    const reload = container.querySelector<HTMLButtonElement>(".boundary-reload");
    if (reload === null) throw new Error("root reload control did not render");
    await act(async () => reload.click());

    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(container.textContent).toContain("healthy app tree");
  });

  it("logs once and still resets under StrictMode double mount", async () => {
    let broken = true;
    function Flaky() {
      if (broken) throw new Error("strict render failed");
      return <main>strict healthy tree</main>;
    }

    await act(async () => {
      root.render(
        <StrictMode>
          <ReloadHarness>
            <Flaky />
          </ReloadHarness>
        </StrictMode>,
      );
    });
    expect(container.querySelector('[role="alert"]')).not.toBeNull();
    // componentDidCatch runs once per caught error, not once per render.
    expect(boundaryLogs()).toHaveLength(1);

    broken = false;
    const reload = container.querySelector<HTMLButtonElement>(".boundary-reload");
    if (reload === null) throw new Error("root reload control did not render");
    await act(async () => reload.click());

    expect(container.textContent).toContain("strict healthy tree");
  });
});
