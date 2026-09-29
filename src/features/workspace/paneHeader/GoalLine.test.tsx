// The goal row under the pane header: nothing without a goal, the full
// objective one line with an ellipsis, and the chevron — shown only while
// the text overflows — that expands the row inline. happy-dom computes no
// layout, so the geometry is stubbed and fired by hand; the overflow itself
// stays a live-app check.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { GoalLine } from "./GoalLine";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** happy-dom's ResizeObserver never fires, so the live cases stand in one
 * that captures its callback: firing it is the goal text resizing. */
class CapturingObserver {
  static live: CapturingObserver[] = [];
  callback: ResizeObserverCallback;
  watched: Element[] = [];
  released = false;
  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
    CapturingObserver.live.push(this);
  }
  observe(target: Element): void {
    this.watched.push(target);
  }
  unobserve(): void {}
  disconnect(): void {
    this.released = true;
  }
  fire(): void {
    this.callback([], this as unknown as ResizeObserver);
  }
}

const GOAL = "Move checkout to the provider registry without a slow first call";

let container: HTMLDivElement;
let root: Root | null;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  CapturingObserver.live = [];
  vi.stubGlobal("ResizeObserver", CapturingObserver);
});

afterEach(async () => {
  vi.unstubAllGlobals();
  if (root !== null) await act(async () => root?.unmount());
  container.remove();
});

function observer(): CapturingObserver {
  const found = CapturingObserver.live[CapturingObserver.live.length - 1];
  if (found === undefined) throw new Error("goal line observed nothing");
  return found;
}

function overflowText(wide: boolean): void {
  const text = container.querySelector(".goal-line-text");
  if (text === null) throw new Error("goal text did not render");
  Object.defineProperty(text, "clientWidth", { value: 400, configurable: true });
  Object.defineProperty(text, "scrollWidth", {
    value: wide ? 800 : 400,
    configurable: true,
  });
}

async function renderGoal(goal: string | null): Promise<void> {
  if (root === null) throw new Error("root was not created");
  await act(async () => {
    root?.render(<GoalLine goal={goal} />);
  });
}

describe("the goal row", () => {
  it("renders no row without a goal", async () => {
    await renderGoal(null);

    expect(container.querySelector('[data-testid="goal-line"]')).toBeNull();
  });

  it("shows the label and the objective on one element with its full text", async () => {
    await renderGoal(GOAL);

    const row = container.querySelector('[data-testid="goal-line"]');
    expect(row).not.toBeNull();
    expect(row?.textContent).toContain("Goal");
    const text = row?.querySelector(".goal-line-text");
    expect(text?.textContent).toBe(GOAL);
    // The collapsed line keeps the whole objective available: hover and
    // assistive tech read the same full text the ellipsis hides.
    expect(text?.getAttribute("title")).toBe(GOAL);
  });

  it("shows no chevron until the text overflows", async () => {
    await renderGoal(GOAL);
    overflowText(false);
    act(() => observer().fire());

    expect(container.querySelector('[data-testid="goal-line-toggle"]')).toBeNull();
  });

  it("shows the chevron once the text overflows", async () => {
    await renderGoal(GOAL);
    overflowText(true);
    act(() => observer().fire());

    expect(container.querySelector('[data-testid="goal-line-toggle"]')).not.toBeNull();
  });

  it("expands and collapses inline with aria-expanded", async () => {
    await renderGoal(GOAL);
    overflowText(true);
    act(() => observer().fire());

    const toggle = container.querySelector<HTMLButtonElement>('[data-testid="goal-line-toggle"]');
    if (toggle === null) throw new Error("chevron did not render");
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(toggle.getAttribute("aria-label")).toBe("Show the whole goal");

    await act(async () => toggle.click());
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(toggle.getAttribute("aria-label")).toBe("Collapse the goal");
    expect(container.querySelector('[data-testid="goal-line"]')?.classList).toContain(
      "is-expanded",
    );

    await act(async () => toggle.click());
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(container.querySelector('[data-testid="goal-line"]')?.classList).not.toContain(
      "is-expanded",
    );
  });

  it("keeps the chevron while expanded so the row can collapse", async () => {
    await renderGoal(GOAL);
    overflowText(true);
    act(() => observer().fire());

    const toggle = container.querySelector<HTMLButtonElement>('[data-testid="goal-line-toggle"]');
    if (toggle === null) throw new Error("chevron did not render");
    await act(async () => toggle.click());

    // A wrapped goal no longer overflows: the row must still offer collapse.
    overflowText(false);
    act(() => observer().fire());
    expect(container.querySelector('[data-testid="goal-line-toggle"]')).not.toBeNull();
  });

  it("disconnects its observer on unmount", async () => {
    await renderGoal(GOAL);
    const seen = observer();

    await act(async () => root?.unmount());
    root = null;

    expect(seen.released).toBe(true);
  });
});
