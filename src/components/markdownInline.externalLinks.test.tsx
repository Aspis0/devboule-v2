// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { parseMarkdownText } from "./markdownParser";

// Only the hand-off is mocked: the real predicate decides which URL a click
// routes, which is the behaviour these tests are about.
vi.mock("../lib/openInBrowser", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/openInBrowser")>();
  return { ...actual, openInBrowser: vi.fn() };
});

import { openInBrowser } from "../lib/openInBrowser";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: ReturnType<typeof createRoot> | null = null;
let host: HTMLDivElement;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  host.remove();
  vi.mocked(openInBrowser).mockClear();
});

async function render(text: string): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(<div>{parseMarkdownText(text)}</div>));
  return host;
}

describe("external links in markdown", () => {
  it("hands an http(s) link to the system browser without navigating", async () => {
    const container = await render("see [docs](https://e.com/a)");
    const link = container.querySelector<HTMLAnchorElement>("a[href]");
    if (link === null) throw new Error("markdown link did not render");
    const click = new MouseEvent("click", { bubbles: true, cancelable: true });
    await act(async () => link.dispatchEvent(click));
    expect(vi.mocked(openInBrowser)).toHaveBeenCalledWith("https://e.com/a");
    expect(click.defaultPrevented).toBe(true);
  });

  it("hands a middle click to the system browser too", async () => {
    const container = await render("see [docs](https://e.com/a)");
    const link = container.querySelector<HTMLAnchorElement>("a[href]");
    if (link === null) throw new Error("markdown link did not render");
    const middle = new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 });
    await act(async () => link.dispatchEvent(middle));
    expect(vi.mocked(openInBrowser)).toHaveBeenCalledWith("https://e.com/a");
    expect(middle.defaultPrevented).toBe(true);
  });

  it("renders a credentialed link as the text the agent wrote", async () => {
    const container = await render("see [docs](https://user:pass@example.com/a)");
    expect(container.querySelector("a[href]")).toBeNull();
    expect(container.textContent).toBe("see [docs](https://user:pass@example.com/a)");
    await act(async () =>
      container.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true })),
    );
    expect(vi.mocked(openInBrowser)).not.toHaveBeenCalled();
  });

  it("leaves any other mouse button alone", async () => {
    const container = await render("see [docs](https://e.com/a)");
    const link = container.querySelector<HTMLAnchorElement>("a[href]");
    if (link === null) throw new Error("markdown link did not render");
    const secondary = new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 2 });
    await act(async () => link.dispatchEvent(secondary));
    expect(secondary.defaultPrevented).toBe(false);
    expect(vi.mocked(openInBrowser)).not.toHaveBeenCalled();
  });

  it.each([
    ["over the byte ceiling", `https://e.com/${"a".repeat(8192)}`],
    ["malformed", "https://[::1"],
    ["holding a no-break space", "https://e.com/a b"],
  ])("renders a destination %s as the text the agent wrote", async (_name, destination) => {
    const written = `see [docs](${destination})`;
    const container = await render(written);
    expect(container.querySelector("a[href]")).toBeNull();
    expect(container.textContent).toBe(written);
  });

  it("leaves a mailto click to the browser", async () => {
    const container = await render("write to [them](mailto:someone@example.com)");
    const link = container.querySelector<HTMLAnchorElement>("a[href]");
    if (link === null) throw new Error("mailto link did not render");
    const click = new MouseEvent("click", { bubbles: true, cancelable: true });
    await act(async () => link.dispatchEvent(click));
    expect(click.defaultPrevented).toBe(false);
    expect(vi.mocked(openInBrowser)).not.toHaveBeenCalled();
  });
});
