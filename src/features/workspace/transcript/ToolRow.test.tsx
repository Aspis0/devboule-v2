// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { ToolRow } from "./ToolRow";

// Only the hand-off is mocked: the real predicate decides which URL a click
// routes, which is the behaviour these tests are about.
vi.mock("../../../lib/openInBrowser", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../lib/openInBrowser")>();
  return { ...actual, openInBrowser: vi.fn() };
});

import { openInBrowser } from "../../../lib/openInBrowser";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function tool(overrides: Partial<ToolChatItem> = {}): ToolChatItem {
  return {
    id: "tool-1",
    role: "tool",
    title: "",
    output: "",
    toolCallId: "t1",
    status: "completed",
    ...overrides,
  };
}

let root: ReturnType<typeof createRoot> | null = null;
let host: HTMLDivElement;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  host.remove();
  vi.mocked(openInBrowser).mockClear();
});

async function renderRow(item: ToolChatItem): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(<ToolRow item={item} transcriptEnded={false} />));
  return host;
}

/** The person opens the line: its link is in the body, which mounts only once opened. */
async function openLine(container: HTMLElement): Promise<void> {
  const details = container.querySelector("details");
  if (details === null) throw new Error("the row had nothing to open");
  await act(async () => {
    details.open = true;
    details.dispatchEvent(new Event("toggle"));
  });
}

describe("ToolRow", () => {
  it("collapses a fetched URL to its domain and links it in the body", async () => {
    const container = await renderRow(
      tool({ kind: "fetch", title: "https://docs.example.com/guide?q=1" }),
    );
    expect(container.querySelector(".workspace-chat-tool-summary-text")?.textContent).toBe(
      "docs.example.com",
    );
    await openLine(container);
    const link = container.querySelector<HTMLAnchorElement>(".workspace-chat-tool-link a");
    expect(link?.getAttribute("href")).toBe("https://docs.example.com/guide?q=1");
    expect(link?.getAttribute("target")).toBe("_blank");
    expect(link?.getAttribute("rel")).toBe("noreferrer");
    expect(link?.textContent).toBe("https://docs.example.com/guide?q=1");
  });

  it("hands a fetched URL to the system browser without navigating", async () => {
    const container = await renderRow(
      tool({ kind: "fetch", title: "https://docs.example.com/guide?q=1" }),
    );
    await openLine(container);
    const link = container.querySelector<HTMLAnchorElement>(".workspace-chat-tool-link a");
    if (link === null) throw new Error("fetch row did not render a link");
    const click = new MouseEvent("click", { bubbles: true, cancelable: true });
    await act(async () => link.dispatchEvent(click));
    expect(vi.mocked(openInBrowser)).toHaveBeenCalledWith("https://docs.example.com/guide?q=1");
    expect(click.defaultPrevented).toBe(true);
  });

  it("hands a middle click on the fetched URL to the system browser", async () => {
    const container = await renderRow(
      tool({ kind: "fetch", title: "https://docs.example.com/guide?q=1" }),
    );
    await openLine(container);
    const link = container.querySelector<HTMLAnchorElement>(".workspace-chat-tool-link a");
    if (link === null) throw new Error("fetch row did not render a link");
    const middle = new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 });
    await act(async () => link.dispatchEvent(middle));
    expect(vi.mocked(openInBrowser)).toHaveBeenCalledWith("https://docs.example.com/guide?q=1");
    expect(middle.defaultPrevented).toBe(true);
  });

  it("keeps a credentialed URL's userinfo out of the document and links nothing", async () => {
    const container = await renderRow(
      tool({ kind: "fetch", title: "https://user:pass@example.com/path" }),
    );
    expect(container.querySelector(".workspace-chat-tool-summary-text")?.textContent).toBe(
      "example.com",
    );
    expect(container.querySelector(".workspace-chat-tool-link")).toBeNull();
    expect(container.innerHTML).not.toContain("user:pass");
    expect(container.innerHTML).not.toContain("user%3Apass");
  });

  it("keeps a URL plus extra text raw and unlinked", async () => {
    const container = await renderRow(
      tool({ kind: "fetch", title: "https://example.com/path explanation" }),
    );
    expect(container.querySelector(".workspace-chat-tool-summary-text")?.textContent).toBe(
      "https://example.com/path explanation",
    );
    expect(container.querySelectorAll("a[href]")).toHaveLength(0);
  });

  it("links nothing for a fetched URL the command would refuse", async () => {
    const title = `https://docs.example.com/${"a".repeat(8192)}`;
    const container = await renderRow(tool({ kind: "fetch", title }));
    expect(container.querySelector(".workspace-chat-tool-link")).toBeNull();
    expect(container.querySelectorAll("a[href]")).toHaveLength(0);
  });

  it("shows a fetched page title and links nothing when the title is not a URL", async () => {
    const container = await renderRow(tool({ kind: "fetch", title: "Rust testing guide" }));
    expect(container.querySelector(".workspace-chat-tool-summary-text")?.textContent).toBe(
      "Rust testing guide",
    );
    expect(container.querySelector(".workspace-chat-tool-link")).toBeNull();
  });

  it("shows a fetch with no title without a summary or a link", async () => {
    const container = await renderRow(tool({ kind: "fetch", title: "" }));
    expect(container.querySelector(".workspace-chat-tool-summary-text")).toBeNull();
    expect(container.querySelector(".workspace-chat-tool-link")).toBeNull();
  });

  it("keeps a search row's query and lists no sources", async () => {
    const container = await renderRow(
      tool({
        kind: "search",
        title: "how to test rust",
        output: "Web search results for query: how to test rust",
      }),
    );
    expect(container.querySelector(".workspace-chat-tool-summary-text")?.textContent).toBe(
      "how to test rust",
    );
    expect(container.querySelectorAll("a[href]")).toHaveLength(0);
  });

  it("keeps the generic fallback row unchanged", async () => {
    const container = await renderRow(tool({ kind: "other", title: "custom thing happened" }));
    expect(container.querySelector(".workspace-chat-tool-label")?.textContent).toBe("Tool");
    expect(container.querySelector(".workspace-chat-tool-summary-text")?.textContent).toBe(
      "custom thing happened",
    );
    expect(container.querySelectorAll("a[href]")).toHaveLength(0);
  });
});
