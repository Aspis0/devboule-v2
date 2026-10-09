// @vitest-environment happy-dom

// A tool call is one quiet line: an icon, a verb, its target. Nothing else
// stands on it when the call went well; its output waits behind the line.

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
let host: HTMLDivElement | null = null;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  host?.remove();
  host = null;
});

async function renderRow(item: ToolChatItem): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(<ToolRow item={item} transcriptEnded={false} />));
  return host;
}

const lines = (count: number, prefix = "line") =>
  Array.from({ length: count }, (_, index) => `${prefix} ${index + 1}`).join("\n");

describe("ToolRow line", () => {
  it("is an icon, a verb and a target, with no check mark when the call went well", async () => {
    const container = await renderRow(
      tool({ kind: "read", title: "scout/BUGS.md", locations: [{ path: "scout/BUGS.md" }] }),
    );
    const summary = container.querySelector(".workspace-chat-tool-summary");
    expect(summary?.querySelector(".workspace-chat-tool-icon")?.getAttribute("aria-hidden")).toBe(
      "true",
    );
    expect(container.querySelector(".workspace-chat-tool-label")?.textContent).toBe("Read");
    expect(container.querySelector(".workspace-chat-tool-summary-text")?.textContent).toBe(
      "scout/BUGS.md",
    );
    expect(container.querySelector(".workspace-chat-tool-done")).toBeNull();
    expect(container.textContent).not.toContain("✓");
  });

  it("names an edit by its verb and file, with no added and removed counts on the line", async () => {
    const container = await renderRow(
      tool({
        kind: "edit",
        title: "src/a.ts",
        locations: [{ path: "src/a.ts" }],
        output: "@@ f\n- old\n+ new\n+ newer",
      }),
    );
    expect(container.querySelector(".workspace-chat-tool-label")?.textContent).toBe("Edit");
    expect(container.querySelector(".workspace-chat-tool-summary-text")?.textContent).toBe(
      "src/a.ts",
    );
    expect(container.querySelector(".workspace-chat-tool-stat")).toBeNull();
  });

  it("never prints a count of hidden lines on the line itself", async () => {
    const container = await renderRow(tool({ kind: "search", title: "q", output: lines(10) }));
    expect(container.querySelector(".workspace-chat-tool-summary")?.textContent).not.toMatch(
      /\+\d+ lines?/,
    );
  });

  it("shows a terminal key call as Send keys to terminal, not as its tool name", async () => {
    const container = await renderRow(
      tool({ kind: "other", title: "devboule_send_terminal_keys" }),
    );
    expect(container.querySelector(".workspace-chat-tool-label")?.textContent).toBe(
      "Send keys to terminal",
    );
  });

  it("shows a browser navigation as Navigate and its address", async () => {
    const container = await renderRow(
      tool({ kind: "browser", title: "navigate https://example.test/cart" }),
    );
    expect(container.querySelector(".workspace-chat-tool-label")?.textContent).toBe("Navigate");
    expect(container.querySelector(".workspace-chat-tool-summary-text")?.textContent).toBe(
      "https://example.test/cart",
    );
  });

  it("keeps the output closed until the person opens the line", async () => {
    const container = await renderRow(
      tool({ kind: "execute", title: "ls", command: "ls", output: lines(4) }),
    );
    const details = container.querySelector("details");
    if (details === null) throw new Error("the call had output but no line to open it");
    expect(details.open).toBe(false);
    expect(container.querySelector(".workspace-chat-tool-output-line")).toBeNull();
    expect(container.querySelector(".workspace-chat-tool-summary")?.tagName).toBe("SUMMARY");
  });

  it("keeps a failed call's mark and its short excerpt on the line", async () => {
    const container = await renderRow(
      tool({
        kind: "execute",
        title: "t",
        command: "t",
        status: "failed",
        output: lines(5, "err"),
      }),
    );
    expect(container.querySelector(".workspace-chat-tool-failed")?.textContent).toContain("failed");
    expect(
      container.querySelectorAll(
        ".workspace-chat-tool-output.is-failure .workspace-chat-tool-output-line",
      ),
    ).toHaveLength(3);
  });

  it("shows a non-zero exit as a failure mark and its number, and a zero exit as nothing", async () => {
    const ok = await renderRow(tool({ kind: "execute", title: "ls", command: "ls", exitCode: 0 }));
    expect(ok.querySelector(".workspace-command-exit")).toBeNull();
    await act(async () => root?.unmount());
    root = null;
    host?.remove();

    const failed = await renderRow(
      tool({ kind: "execute", title: "ls", command: "ls", exitCode: 2 }),
    );
    expect(failed.querySelector(".workspace-command-exit")?.textContent).toBe("exit 2");
    expect(failed.querySelector(".workspace-command-dot")).not.toBeNull();
  });
});
