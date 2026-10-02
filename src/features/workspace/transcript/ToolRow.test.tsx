// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { ToolRow } from "./ToolRow";

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
});

async function renderRow(item: ToolChatItem): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(<ToolRow item={item} transcriptEnded={false} />));
  return host;
}

describe("ToolRow", () => {
  it("collapses a fetched URL to its domain and links it in the body", async () => {
    const container = await renderRow(
      tool({ kind: "fetch", title: "https://docs.example.com/guide?q=1" }),
    );
    expect(container.querySelector(".workspace-chat-tool-summary-text")?.textContent).toBe(
      "docs.example.com",
    );
    const link = container.querySelector<HTMLAnchorElement>(".workspace-chat-tool-link a");
    expect(link?.getAttribute("href")).toBe("https://docs.example.com/guide?q=1");
    expect(link?.getAttribute("target")).toBe("_blank");
    expect(link?.getAttribute("rel")).toBe("noreferrer");
    expect(link?.textContent).toBe("https://docs.example.com/guide?q=1");
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
