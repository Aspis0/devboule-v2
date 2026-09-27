// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useRef } from "react";
import { useStripFade } from "./useStripFade";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const TABS = [{ id: "a" }, { id: "b" }];

function Harness({ tabs, onRender }: { tabs: readonly { id: string }[]; onRender: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const fade = useStripFade(ref, tabs);
  onRender();
  return (
    <div
      ref={ref}
      data-fade-left={fade.left ? "true" : "false"}
      data-fade-right={fade.right ? "true" : "false"}
    />
  );
}

let root: ReturnType<typeof createRoot> | null = null;
let container: HTMLDivElement | null = null;

function mount(tabs: readonly { id: string }[], onRender: () => void) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root!.render(<Harness tabs={tabs} onRender={onRender} />);
  });
  return container.querySelector<HTMLDivElement>("div")!;
}

function rerender(tabs: readonly { id: string }[], onRender: () => void) {
  act(() => {
    root!.render(<Harness tabs={tabs} onRender={onRender} />);
  });
  return container!.querySelector<HTMLDivElement>("div")!;
}

afterEach(() => {
  act(() => {
    root?.unmount();
  });
  container?.remove();
  root = null;
  container = null;
  vi.clearAllMocks();
});

describe("useStripFade", () => {
  it("does not re-render when a scroll frame changes nothing", async () => {
    let renders = 0;
    const el = mount(TABS, () => {
      renders += 1;
    });
    Object.defineProperties(el, {
      scrollWidth: { value: 200, configurable: true },
      clientWidth: { value: 200, configurable: true },
      scrollLeft: { value: 0, writable: true, configurable: true },
    });
    const settled = renders;
    act(() => {
      el.dispatchEvent(new Event("scroll", { bubbles: true }));
      el.dispatchEvent(new Event("scroll", { bubbles: true }));
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    expect(renders).toBe(settled);
    expect(el.dataset.fadeRight).toBe("false");
  });

  it("coalesces a burst of scroll events into one read", async () => {
    let renders = 0;
    const el = mount(TABS, () => {
      renders += 1;
    });
    let reads = 0;
    Object.defineProperties(el, {
      scrollWidth: {
        get: () => {
          reads += 1;
          return 500;
        },
        configurable: true,
      },
      clientWidth: { value: 200, configurable: true },
      scrollLeft: { value: 0, writable: true, configurable: true },
    });
    reads = 0;
    act(() => {
      el.dispatchEvent(new Event("scroll", { bubbles: true }));
      el.dispatchEvent(new Event("scroll", { bubbles: true }));
      el.dispatchEvent(new Event("scroll", { bubbles: true }));
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    expect(reads).toBe(1);
    expect(el.dataset.fadeRight).toBe("true");
  });

  it("re-reads when the roster changes without any scroll event", () => {
    let renders = 0;
    const el = mount([{ id: "a" }], () => {
      renders += 1;
    });
    Object.defineProperties(el, {
      scrollWidth: { value: 500, configurable: true },
      clientWidth: { value: 200, configurable: true },
      scrollLeft: { value: 0, writable: true, configurable: true },
    });
    expect(el.dataset.fadeRight).toBe("false");
    rerender([...TABS, { id: "c" }], () => {
      renders += 1;
    });
    expect(el.dataset.fadeRight).toBe("true");
    expect(renders).toBeGreaterThan(0);
  });
});
