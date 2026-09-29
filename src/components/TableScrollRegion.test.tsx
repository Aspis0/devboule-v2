// The wrapper is a plain div until the table actually overflows it, then a
// focusable region named from the header cells, and back again; the
// observer goes with the wrapper. happy-dom computes no layout, so the
// geometry is stubbed and fired by hand — the overflow itself stays a
// live-app check.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { parseMarkdownText } from "./markdownParser";
import { TableScrollRegion } from "./TableScrollRegion";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** happy-dom's ResizeObserver never fires, so the live cases stand in one
 * that captures its callback: firing it is the wrapper or table resizing. */
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

interface Harness {
  root: Root;
  container: HTMLElement;
  wrapper: HTMLElement;
  table: Element;
  observer: CapturingObserver;
  overflow: (wide: boolean) => void;
}

async function renderRegion(headers: string[]): Promise<Harness> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      <TableScrollRegion headers={headers}>
        <table className="plan-markdown-table">
          <tbody>
            <tr>
              <td>cell</td>
            </tr>
          </tbody>
        </table>
      </TableScrollRegion>,
    );
  });
  const wrapper = container.querySelector<HTMLElement>(".plan-markdown-table-scroll");
  if (wrapper === null || wrapper.firstElementChild === null) {
    throw new Error("table scroll wrapper did not render");
  }
  return {
    root,
    container,
    wrapper,
    table: wrapper.firstElementChild,
    observer: CapturingObserver.live[CapturingObserver.live.length - 1],
    overflow: (wide) => {
      Object.defineProperty(wrapper, "clientWidth", { value: 500, configurable: true });
      Object.defineProperty(wrapper.firstElementChild, "scrollWidth", {
        value: wide ? 800 : 400,
        configurable: true,
      });
    },
  };
}

describe("the table scroll region", () => {
  beforeEach(() => {
    CapturingObserver.live = [];
    globalThis.ResizeObserver = CapturingObserver as unknown as typeof ResizeObserver;
  });

  afterEach(async () => {
    document.body.innerHTML = "";
  });

  it("renders a plain wrapper until a measurement says otherwise", async () => {
    const region = await renderRegion(["Name", "Value"]);

    expect(region.wrapper.hasAttribute("tabindex")).toBe(false);
    expect(region.wrapper.getAttribute("role")).toBeNull();
    expect(region.wrapper.getAttribute("aria-label")).toBeNull();
    expect(region.observer.watched).toEqual([region.wrapper, region.table]);
    await act(async () => region.root.unmount());
  });

  it("becomes a region named from the header cells while the table overflows", async () => {
    const region = await renderRegion(["Name", "Size", "Kind", "Extra"]);

    region.overflow(true);
    await act(async () => region.observer.fire());

    expect(region.wrapper.getAttribute("role")).toBe("region");
    expect(region.wrapper.getAttribute("tabindex")).toBe("0");
    expect(region.wrapper.getAttribute("aria-label")).toBe("Table: Name, Size, Kind");
    await act(async () => region.root.unmount());
  });

  it("names the region Table when no header has text", async () => {
    const region = await renderRegion(["", "   "]);

    region.overflow(true);
    await act(async () => region.observer.fire());

    expect(region.wrapper.getAttribute("aria-label")).toBe("Table");
    await act(async () => region.root.unmount());
  });

  it("falls back to a plain wrapper when the overflow ends", async () => {
    const region = await renderRegion(["Name"]);
    region.overflow(true);
    await act(async () => region.observer.fire());
    expect(region.wrapper.getAttribute("role")).toBe("region");

    region.overflow(false);
    await act(async () => region.observer.fire());

    expect(region.wrapper.hasAttribute("tabindex")).toBe(false);
    expect(region.wrapper.getAttribute("role")).toBeNull();
    await act(async () => region.root.unmount());
  });

  it("drops the observer on unmount", async () => {
    const region = await renderRegion(["Name"]);
    region.overflow(true);
    await act(async () => region.observer.fire());

    await act(async () => region.root.unmount());

    expect(region.observer.released).toBe(true);
    expect(() => region.observer.fire()).not.toThrow();
  });

  it("renders a plain table when ResizeObserver is missing", async () => {
    const saved = globalThis.ResizeObserver;
    delete (globalThis as { ResizeObserver?: unknown }).ResizeObserver;
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    try {
      await act(async () => {
        root.render(
          <TableScrollRegion headers={["Name"]}>
            <table>
              <tbody>
                <tr>
                  <td>cell</td>
                </tr>
              </tbody>
            </table>
          </TableScrollRegion>,
        );
      });
      expect(container.querySelector(".plan-markdown-table-scroll")).not.toBeNull();
      expect(container.querySelector("table")).not.toBeNull();
    } finally {
      globalThis.ResizeObserver = saved;
      await act(async () => root.unmount());
      container.remove();
    }
  });

  it("names the region from the finished header names it is given", async () => {
    const region = await renderRegion(["bold", "code", "plain"]);
    region.overflow(true);
    await act(async () => region.observer.fire());
    expect(region.wrapper.getAttribute("aria-label")).toBe("Table: bold, code, plain");
    await act(async () => region.root.unmount());
  });

  // The spoken name must be the column's own text: one shape per inline
  // construct the parser renders, or deliberately leaves alone.
  it.each([
    "Plain",
    "**bold**",
    "`code`",
    "_em_",
    "__init__",
    "***x***",
    "\\*literal\\*",
    "\\_escaped\\_",
    "[a](https://e.com/x(y))",
    "[**b**](http://e.com)",
    "![](http://e.com)",
    "**a _b_ c**",
    "a*b",
    "~~strike~~",
    "![alt](http://e.com/i.png)",
    "[a](http://e.com)",
  ])("names %j like its column shows", async (source) => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    try {
      await act(async () => {
        root.render(<div>{parseMarkdownText(`| ${source} |\n|---|\n| x |`)}</div>);
      });
      const wrapper = container.querySelector<HTMLElement>(".plan-markdown-table-scroll");
      const th = container.querySelector("th");
      if (wrapper === null || th === null) throw new Error(`no table for ${source}`);
      Object.defineProperty(wrapper, "clientWidth", { value: 500, configurable: true });
      Object.defineProperty(wrapper.firstElementChild, "scrollWidth", {
        value: 800,
        configurable: true,
      });
      const observer = CapturingObserver.live[CapturingObserver.live.length - 1];
      await act(async () => observer.fire());
      const shown = th.textContent ?? "";
      expect(wrapper.getAttribute("aria-label")).toBe(shown === "" ? "Table" : `Table: ${shown}`);
    } finally {
      await act(async () => root.unmount());
      container.remove();
    }
  });
});
