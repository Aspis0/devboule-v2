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
  let clipboardWrites: string[];
  const realClipboard = navigator.clipboard;

  beforeEach(() => {
    clipboardWrites = [];
    // happy-dom ships a clipboard the tests must not reach into, so the
    // property is replaced and restored rather than spied on.
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (text: string) => {
          clipboardWrites.push(text);
        },
      },
    });
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
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: realClipboard });
    warnSpy.mockRestore();
    errorSpy.mockRestore();
    const location = window.location as unknown as Record<string, unknown>;
    if (Object.hasOwn(location, "reload")) delete location.reload;
  });

  function surfaceFallback(): HTMLElement {
    const fallback = container.querySelector<HTMLElement>(".surface-fallback");
    if (fallback === null) throw new Error("the surface fallback did not render");
    return fallback;
  }

  async function renderBroken(message: string): Promise<HTMLElement> {
    function Broken(): ReactNode {
      throw new Error(message);
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
    return surfaceFallback();
  }

  it("announces a plain sentence and keeps the exception behind a closed disclosure", async () => {
    const fallback = await renderBroken("surface render failed");

    const live = fallback.querySelector('[role="alert"]');
    expect(live?.textContent).toBe("Workspace stopped working.");

    const details = fallback.querySelector("details");
    if (details === null) throw new Error("the technical details disclosure did not render");
    expect(details.open).toBe(false);
    expect(details.querySelector("summary")?.textContent).toBe("Technical details");

    // The sentence and the actions are all the user sees until the disclosure
    // is opened: drop it from a clone and nothing left may carry the throw.
    const withoutDetails = fallback.cloneNode(true) as HTMLElement;
    withoutDetails.querySelector("details")?.remove();
    expect(withoutDetails.textContent).not.toContain("surface render failed");
    expect(withoutDetails.textContent).not.toContain("Error");

    // The shell around the broken surface keeps working.
    expect(fallback.querySelector(".boundary-retry")?.textContent).toBe("Retry");
    expect(fallback.querySelector(".surface-fallback-reload")?.textContent).toBe("Reload Devboule");
    expect(container.querySelector(".shell-stand-in")?.textContent).toBe("sidebar");
    expect(warnSpy).toHaveBeenCalledWith(
      "[SurfaceErrorBoundary] Surface render failed",
      "Workspace",
      expect.any(Error),
      expect.anything(),
    );
  });

  it("keeps a semantic heading and announces the sentence through its own alert", async () => {
    const fallback = await renderBroken("heading probe failed");

    const heading = fallback.querySelector("h2");
    if (heading === null) throw new Error("the heading did not render");
    expect(heading.hasAttribute("role")).toBe(false);
    expect(heading.textContent).toBe("Workspace stopped working.");

    const alerts = fallback.querySelectorAll('[role="alert"]');
    expect(alerts).toHaveLength(1);
    expect(alerts[0]).not.toBe(heading);
    expect(alerts[0]?.textContent).toBe("Workspace stopped working.");
  });

  it("puts the exception message and stack inside the disclosure", async () => {
    const fallback = await renderBroken("detail probe failed");

    const details = fallback.querySelector("details");
    if (details === null) throw new Error("the technical details disclosure did not render");
    expect(details.textContent).toContain("detail probe failed");
    expect(details.textContent).toContain("Error");
  });

  it("copies the technical details for a bug report", async () => {
    const fallback = await renderBroken("copy probe failed");

    const copy = fallback.querySelector<HTMLButtonElement>(".surface-fallback-copy");
    if (copy === null) throw new Error("the copy control did not render");
    expect(copy.textContent).toBe("Copy details");

    await act(async () => copy.click());
    await act(async () => undefined);

    expect(clipboardWrites).toEqual([
      fallback.querySelector<HTMLPreElement>(".surface-fallback-details")?.textContent,
    ]);
    expect(clipboardWrites[0]).toContain("copy probe failed");
    expect(copy.textContent).toBe("Copied");
  });

  it("warns next to Copy details that the text may include local file paths", async () => {
    const fallback = await renderBroken("note probe failed");

    const copy = fallback.querySelector<HTMLButtonElement>(".surface-fallback-copy");
    expect(copy?.parentElement?.textContent).toContain(
      "May include file paths from this computer.",
    );
  });

  it("retry re-mounts the surface: mount and cleanup counts prove it", async () => {
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
    // so the retry below cannot be a re-render of the old instance.
    broken = true;
    await act(async () => {
      root.render(<Harness />);
    });
    expect(container.querySelector(".surface-fallback")).not.toBeNull();
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

  it("a retry that throws again shows the fallback again and never retries by itself", async () => {
    let attempts = 0;
    function Stubborn(): ReactNode {
      attempts += 1;
      throw new Error("still failing");
    }
    await act(async () => {
      root.render(
        <SurfaceErrorBoundary surfaceLabel="Polis">
          <Stubborn />
        </SurfaceErrorBoundary>,
      );
    });
    expect(warnSpy).toHaveBeenCalledTimes(1);

    const before = attempts;
    const retry = container.querySelector<HTMLButtonElement>(".boundary-retry");
    if (retry === null) throw new Error("surface retry control did not render");
    await act(async () => retry.click());

    expect(attempts).toBeGreaterThan(before);
    expect(warnSpy).toHaveBeenCalledTimes(2);
    expect(surfaceFallback().querySelector('[role="alert"]')?.textContent).toBe(
      "Polis stopped working.",
    );

    const settled = attempts;
    await act(async () => new Promise((resolve) => setTimeout(resolve, 50)));
    expect(attempts).toBe(settled);
    expect(warnSpy).toHaveBeenCalledTimes(2);
  });

  describe("focus", () => {
    let broken: boolean;
    function Child(): ReactNode {
      if (broken) throw new Error("focus probe failed");
      return <input className="surface-input" />;
    }
    function Harness(): ReactNode {
      return (
        <>
          <button type="button" className="shell-button">
            sidebar action
          </button>
          <SurfaceErrorBoundary surfaceLabel="Workspace">
            <Child />
          </SurfaceErrorBoundary>
        </>
      );
    }

    beforeEach(() => {
      broken = false;
    });

    function focusable(selector: string): HTMLElement {
      const element = container.querySelector<HTMLElement>(selector);
      if (element === null) throw new Error(`${selector} did not render`);
      return element;
    }

    async function breakSurface(): Promise<void> {
      broken = true;
      await act(async () => {
        root.render(<Harness />);
      });
    }

    it("moves focus to Retry when the focused element was inside the failed surface", async () => {
      await act(async () => {
        root.render(<Harness />);
      });
      focusable(".surface-input").focus();
      expect(document.activeElement).toBe(focusable(".surface-input"));

      await breakSurface();

      expect(document.activeElement).toBe(focusable(".boundary-retry"));
    });

    it("moves focus to Retry when nothing is focused", async () => {
      broken = true;
      await act(async () => {
        root.render(<Harness />);
      });

      expect(document.activeElement).toBe(focusable(".boundary-retry"));
    });

    it("leaves focus alone when it is elsewhere in the app", async () => {
      await act(async () => {
        root.render(<Harness />);
      });
      focusable(".shell-button").focus();

      await breakSurface();

      expect(document.activeElement).toBe(focusable(".shell-button"));
    });

    it("returns focus to Retry after a retry that throws again", async () => {
      broken = true;
      await act(async () => {
        root.render(<Harness />);
      });
      await act(async () => focusable(".boundary-retry").click());

      expect(document.activeElement).toBe(focusable(".boundary-retry"));
    });
  });

  describe("error details formatting", () => {
    async function detailsOf(thrown: unknown): Promise<string> {
      function Broken(): ReactNode {
        throw thrown;
      }
      await act(async () => {
        root.render(
          <SurfaceErrorBoundary surfaceLabel="Workspace">
            <Broken />
          </SurfaceErrorBoundary>,
        );
      });
      return surfaceFallback().querySelector(".surface-fallback-details")?.textContent ?? "";
    }

    it("formats a circular object without crashing the fallback", async () => {
      const circular: Record<string, unknown> = {};
      circular.self = circular;

      expect(await detailsOf(circular)).toBe("Unrenderable error value");
    });

    it("formats an Error whose getters throw without crashing the fallback", async () => {
      const hostile = new Error("hostile");
      for (const key of ["stack", "name", "message"] as const) {
        Object.defineProperty(hostile, key, {
          get() {
            throw new Error(`${key} getter threw`);
          },
        });
      }

      expect(await detailsOf(hostile)).toBe("Unrenderable error value");
    });

    it("formats a thrown string as itself", async () => {
      expect(await detailsOf("plain failure")).toBe("plain failure");
    });
  });

  it("a rejected lazy import shows the fallback and its Reload calls location.reload", async () => {
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
    const fallback = surfaceFallback();
    expect(fallback.querySelector('[role="alert"]')?.textContent).toBe("Settings stopped working.");
    expect(fallback.querySelector("details")?.textContent).toContain("chunk load failed");

    const reloadButton = fallback.querySelector<HTMLButtonElement>(".surface-fallback-reload");
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
    expect(container.querySelector(".surface-fallback")).not.toBeNull();

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
