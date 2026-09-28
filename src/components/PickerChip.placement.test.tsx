// The picker menu's side and cap (menuPlacement): above the trigger with the
// room above when the content fits there, below it with that side's room when
// it does not, measured with the cap lifted. The direct cases pin the
// The direct cases pin the
// arithmetic; the React cases pin that the side reaches the inline style
// and that an open menu recomputes while the window or its rows move.
// @vitest-environment happy-dom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { assembleCssProof } from "../features/workspace/cssProof";
import { menuPlacement, PickerChip } from "./PickerChip";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../..");
const SHEETS = [
  readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/components/PickerChip.css"), "utf8"),
];

function makeRect(left: number, top: number, right: number, bottom: number): DOMRect {
  return {
    left,
    top,
    right,
    bottom,
    width: right - left,
    height: bottom - top,
    x: left,
    y: top,
    toJSON: () => undefined,
  } as DOMRect;
}

const OPTIONS = [
  { id: "m1", name: "First", description: "First description" },
  { id: "m2", name: "Second", description: "Second description" },
  { id: "m3", name: "Third", description: "Third description" },
];

/** A trigger inside the clipping panel, and a menu whose content is `natural`
 * px tall once the cap is lifted (`capped` px through it) — the shape the
 * review measured in real Chrome (486 through a 285 px capped box). */
function stubbedPair(
  triggerTop: number,
  natural: number,
  capped: number,
): {
  panel: HTMLElement;
  trigger: HTMLElement;
  menu: HTMLElement;
} {
  const panel = document.createElement("div");
  panel.className = "workspace-center-panel";
  const trigger = document.createElement("button");
  panel.appendChild(trigger);
  document.body.appendChild(panel);
  const menu = document.createElement("div");
  document.body.appendChild(menu);
  panel.getBoundingClientRect = () => makeRect(0, 0, 600, 500);
  trigger.getBoundingClientRect = () => makeRect(50, triggerTop, 150, triggerTop + 28);
  menu.style.maxHeight = "280px";
  Object.defineProperty(menu, "scrollWidth", { value: 220, configurable: true });
  Object.defineProperty(menu, "scrollHeight", {
    get: () => (menu.style.maxHeight === "none" ? natural : capped),
    configurable: true,
  });
  return { panel, trigger, menu };
}

describe("menuPlacement", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("opens above with the room above when the content fits there", () => {
    const { trigger, menu } = stubbedPair(420, 200, 200);
    // Room above the trigger: 420 − margin 8 = 412, minus the anchor gap.
    expect(menuPlacement(trigger, menu)).toEqual({ maxHeight: 406, below: false });
    expect(menu.style.maxHeight).toBe("280px");
  });

  it("opens below with that side's room when the content fits neither side", () => {
    const { trigger, menu } = stubbedPair(60, 486, 285);
    // Above holds 52 px, below 404: the larger side wins, capped at 280.
    expect(menuPlacement(trigger, menu)).toEqual({ maxHeight: 280, below: true });
    expect(menu.style.maxHeight).toBe("280px");
  });

  it("measures the content with the cap lifted, not the capped box", () => {
    const { trigger, menu } = stubbedPair(60, 100, 40);
    // Through the cap the content reads 40 px and would fit above (room 46);
    // lifted it is 100 px and does not — so the menu must open below.
    expect(menuPlacement(trigger, menu)).toEqual({ maxHeight: 280, below: true });
  });

  it("falls back to the window when the chip sits outside the centre panel", () => {
    const widthDesc = Object.getOwnPropertyDescriptor(window, "innerWidth");
    const heightDesc = Object.getOwnPropertyDescriptor(window, "innerHeight");
    Object.defineProperty(window, "innerWidth", { value: 800, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: 600, configurable: true });
    try {
      const trigger = document.createElement("button");
      document.body.appendChild(trigger);
      const menu = document.createElement("div");
      document.body.appendChild(menu);
      trigger.getBoundingClientRect = () => makeRect(50, 40, 150, 68);
      Object.defineProperty(menu, "scrollWidth", { value: 220, configurable: true });
      Object.defineProperty(menu, "scrollHeight", { value: 500, configurable: true });
      // 500 px fits below (room 524) but not above (room 32): below, capped.
      expect(menuPlacement(trigger, menu)).toEqual({ maxHeight: 280, below: true });
    } finally {
      if (widthDesc !== undefined) Object.defineProperty(window, "innerWidth", widthDesc);
      if (heightDesc !== undefined) Object.defineProperty(window, "innerHeight", heightDesc);
    }
  });
});

async function openMenu(
  currentId: string,
  count = OPTIONS.length,
): Promise<{
  root: Root;
  container: HTMLElement;
  menu: HTMLElement;
  trigger: HTMLButtonElement;
  rerender: (selected: string, n: number) => Promise<void>;
}> {
  const renderChip = (selected: string, n: number): ReactElement => (
    <PickerChip
      label="Model"
      options={OPTIONS.slice(0, n)}
      currentId={selected}
      onSelect={() => undefined}
      chipTestId="model-chip"
      optionTestId={(id) => `model-option-${id}`}
    />
  );
  const container = document.createElement("div");
  container.className = "workspace-center-panel";
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(renderChip(currentId, count));
  });
  const trigger = container.querySelector<HTMLButtonElement>('[data-testid="model-chip"]');
  if (trigger === null) throw new Error("picker trigger did not render");
  await act(async () => trigger.click());
  const menu = container.querySelector<HTMLElement>(".workspace-mode-menu");
  if (menu === null) throw new Error("picker menu did not open");
  return {
    root,
    container,
    menu,
    trigger,
    rerender: async (selected: string, n: number) => {
      await act(async () => {
        root.render(renderChip(selected, n));
      });
    },
  };
}

/** Stub the geometry after open, then re-render to re-run the open effect. */
function stubOpenMenu(
  container: HTMLElement,
  trigger: HTMLElement,
  menu: HTMLElement,
  triggerTop: number,
  natural: (rows: number) => number,
): void {
  container.getBoundingClientRect = () => makeRect(0, 0, 600, 500);
  trigger.getBoundingClientRect = () => makeRect(50, triggerTop, 150, triggerTop + 28);
  Object.defineProperty(menu, "scrollWidth", { value: 220, configurable: true });
  Object.defineProperty(menu, "scrollHeight", {
    get: () => natural(menu.querySelectorAll("[role='option']").length),
    configurable: true,
  });
}

describe("the open menu's inline placement", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("opens above by default, so the frame before the inline write has a side", () => {
    const css = assembleCssProof(SHEETS);
    expect(css.rulesFor(".workspace-mode-menu")).toContain("bottom: calc(100% + 6px)");
  });

  it("opens above the trigger with the room above when the content fits", async () => {
    const { root, container, menu, trigger, rerender } = await openMenu("m1");
    stubOpenMenu(container, trigger, menu, 420, () => 200);
    await rerender("m2", 3);
    expect(menu.style.maxHeight).toBe("406px");
    expect(menu.style.bottom).toBe("calc(100% + 6px)");
    expect(menu.style.top).toBe("auto");
    await act(async () => root.unmount());
  });

  it("opens below the trigger with that side's cap when nothing fits above", async () => {
    const { root, container, menu, trigger, rerender } = await openMenu("m1");
    stubOpenMenu(container, trigger, menu, 60, () => 486);
    await rerender("m2", 3);
    expect(menu.style.maxHeight).toBe("280px");
    expect(menu.style.top).toBe("calc(100% + 6px)");
    expect(menu.style.bottom).toBe("auto");
    await act(async () => root.unmount());
  });

  it("re-measures when rows arrive late under the open menu", async () => {
    const { root, container, menu, trigger, rerender } = await openMenu("m1", 1);
    expect(menu.style.maxHeight).toBe("");
    stubOpenMenu(container, trigger, menu, 420, (rows) => rows * 48);
    // Same selection, more rows: only the row-count dep re-runs the effect.
    await rerender("m1", 3);
    expect(menu.style.maxHeight).toBe("406px");
    await act(async () => root.unmount());
  });
});

/** happy-dom's ResizeObserver never fires, so the live cases stand in one
 * that captures its callback: firing it is the menu changing its own size. */
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

describe("the open menu's live placement", () => {
  const realObserver = globalThis.ResizeObserver;
  beforeEach(() => {
    CapturingObserver.live = [];
    globalThis.ResizeObserver = CapturingObserver as unknown as typeof ResizeObserver;
  });
  afterEach(() => {
    globalThis.ResizeObserver = realObserver;
    document.body.innerHTML = "";
  });

  function liveObserver(): CapturingObserver {
    const observer = CapturingObserver.live[CapturingObserver.live.length - 1];
    if (observer === undefined) throw new Error("placement observer was not created");
    return observer;
  }

  it("recomputes the cap when the window is resized with the menu open", async () => {
    const { root, container, menu, trigger, rerender } = await openMenu("m1");
    stubOpenMenu(container, trigger, menu, 420, () => 200);
    await rerender("m2", 3);
    expect(menu.style.maxHeight).toBe("406px");
    // The window shrinks: the room above the trigger drops to 292 px.
    trigger.getBoundingClientRect = () => makeRect(50, 300, 150, 328);
    await act(async () => {
      window.dispatchEvent(new Event("resize"));
    });
    expect(menu.style.maxHeight).toBe("286px");
    expect(menu.style.bottom).toBe("calc(100% + 6px)");
    await act(async () => root.unmount());
  });

  it("brings the selected row back into view when the rows grow under the open menu", async () => {
    const { root, container, menu, trigger, rerender } = await openMenu("m1");
    let rowHeight = 48;
    stubOpenMenu(container, trigger, menu, 420, (rows) => rows * rowHeight);
    const tops = [0, 48, 96];
    const rows = [...menu.querySelectorAll("[role='option']")];
    rows.forEach((row, index) => {
      Object.defineProperty(row, "offsetTop", {
        get: () => tops[index],
        configurable: true,
      });
      Object.defineProperty(row, "offsetHeight", {
        get: () => rowHeight,
        configurable: true,
      });
    });
    let scrollTop = 0;
    Object.defineProperty(menu, "clientHeight", { value: 80, configurable: true });
    Object.defineProperty(menu, "scrollTop", {
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
      configurable: true,
    });
    // Selecting the third row scrolls it (96-144) into the 80 px window.
    await rerender("m3", 3);
    expect(scrollTop).toBe(64);
    // Same count, taller rows: the third row runs 258-387, off-screen.
    rowHeight = 129;
    tops[1] = 129;
    tops[2] = 258;
    await act(async () => {
      liveObserver().fire();
    });
    expect(menu.style.maxHeight).toBe("406px");
    expect(scrollTop).toBe(307);
    await act(async () => root.unmount());
  });

  it("drops the resize listener and the observer when the menu closes", async () => {
    const { root, container, menu, trigger, rerender } = await openMenu("m1");
    stubOpenMenu(container, trigger, menu, 420, () => 200);
    await rerender("m2", 3);
    expect(menu.style.maxHeight).toBe("406px");
    const observer = liveObserver();
    expect(observer.watched).toContain(menu);
    await act(async () => trigger.click());
    expect(observer.released).toBe(true);
    // A later resize must not touch the closed menu's box.
    trigger.getBoundingClientRect = () => makeRect(50, 300, 150, 328);
    await act(async () => {
      window.dispatchEvent(new Event("resize"));
    });
    expect(menu.style.maxHeight).toBe("406px");
    await act(async () => root.unmount());
  });

  it("drops the resize listener and the observer on unmount", async () => {
    const { root, container, menu, trigger, rerender } = await openMenu("m1");
    stubOpenMenu(container, trigger, menu, 420, () => 200);
    await rerender("m2", 3);
    const observer = liveObserver();
    await act(async () => root.unmount());
    expect(observer.released).toBe(true);
    trigger.getBoundingClientRect = () => makeRect(50, 300, 150, 328);
    await act(async () => {
      window.dispatchEvent(new Event("resize"));
    });
    expect(menu.style.maxHeight).toBe("406px");
  });
});
