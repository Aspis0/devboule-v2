// @vitest-environment happy-dom

// Tests for the session tab swipe: direction decides the act, the threshold
// decides commit versus spring-back.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  SessionTabSwipe,
  SWIPE_COMMIT_PX,
  SWIPE_SLOP_PX,
  shouldCapturePointer,
} from "./SessionTabSwipe";

let container: HTMLDivElement;
let root: Root;

function renderSwipe(onCommit: (direction: "archive" | "delete") => void): void {
  root = createRoot(container);
  act(() => {
    root.render(
      <SessionTabSwipe onCommit={onCommit}>
        <button type="button">shell one</button>
      </SessionTabSwipe>,
    );
  });
}

function content(): HTMLElement {
  const element = container.querySelector<HTMLElement>("[data-testid=session-swipe-content]");
  if (!element) throw new Error("swipe content did not render");
  return element;
}

function drag(element: HTMLElement, fromX: number, toX: number): void {
  // happy-dom's PointerEvent constructor drops clientX, so drive the
  // pointer handlers with MouseEvents carrying the pointer type instead:
  // React reads clientX off the native event either way.
  const at = (type: string, clientX: number) =>
    new window.MouseEvent(type, { bubbles: true, clientX });
  element.dispatchEvent(at("pointerdown", fromX));
  element.dispatchEvent(at("pointermove", (fromX + toX) / 2));
  element.dispatchEvent(at("pointermove", toX));
  element.dispatchEvent(at("pointerup", toX));
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("SessionTabSwipe", () => {
  it("commits archive on a right-to-left drag past the threshold", () => {
    const onCommit = vi.fn();
    renderSwipe(onCommit);
    act(() => {
      drag(content(), 200, 200 - SWIPE_COMMIT_PX - 30);
    });
    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(onCommit).toHaveBeenCalledWith("archive");
  });

  it("commits delete on a left-to-right drag past the threshold", () => {
    const onCommit = vi.fn();
    renderSwipe(onCommit);
    act(() => {
      drag(content(), 200, 200 + SWIPE_COMMIT_PX + 30);
    });
    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(onCommit).toHaveBeenCalledWith("delete");
  });

  it("springs back without committing when released short of the threshold", () => {
    const onCommit = vi.fn();
    renderSwipe(onCommit);
    act(() => {
      drag(content(), 200, 200 - SWIPE_COMMIT_PX + 20);
    });
    expect(onCommit).not.toHaveBeenCalled();
    expect(content().style.transform).toBe("translateX(0px)");
  });

  it("names both acts under the tab, delete as the heavier one", () => {
    renderSwipe(vi.fn());
    expect(container.textContent).toContain("Archive · keeps messages");
    expect(container.textContent).toContain("Delete · destroys the session");
    const underlays = container.querySelectorAll(".session-swipe-underlay");
    expect(underlays.length).toBe(2);
    for (const underlay of underlays) {
      expect(underlay.getAttribute("aria-hidden")).toBe("true");
    }
  });

  it("keeps the tablist owning the tabs through presentational boxes", () => {
    renderSwipe(vi.fn());
    // Both positioning boxes the swipe needs must stay transparent to ARIA
    // ownership: the tab buttons have to remain the tablist's children in
    // the access tree, not grandchildren of generic divs.
    expect(container.querySelector(".session-swipe")?.getAttribute("role")).toBe("presentation");
    expect(container.querySelector(".session-swipe-content")?.getAttribute("role")).toBe(
      "presentation",
    );
  });
});

describe("shouldCapturePointer", () => {
  // What these assert: the pure decision — capture exactly when the press
  // has become a drag, never twice. What they do NOT cover: that a real
  // press-time capture retargets the click in WebView2. happy-dom no-ops
  // capture, so the environment that broke cannot see the breakage; the
  // regression try is the reviewer's CDP rig, not this file.
  it("captures only once the press has become a drag", () => {
    expect(shouldCapturePointer(0, false)).toBe(false);
    expect(shouldCapturePointer(SWIPE_SLOP_PX - 1, false)).toBe(false);
    expect(shouldCapturePointer(SWIPE_SLOP_PX, false)).toBe(true);
    expect(shouldCapturePointer(-SWIPE_SLOP_PX * 10, false)).toBe(true);
  });

  it("never captures twice", () => {
    expect(shouldCapturePointer(SWIPE_SLOP_PX * 10, true)).toBe(false);
    expect(shouldCapturePointer(0, true)).toBe(false);
  });
});
