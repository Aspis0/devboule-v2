// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { parseMarkdownText } from "./markdownParser";

vi.mock("../lib/openInBrowser", () => ({ openInBrowser: vi.fn() }));

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
});
