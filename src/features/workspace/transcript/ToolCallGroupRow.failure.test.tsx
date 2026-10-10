// @vitest-environment happy-dom

// A group is one closed summary row. A failure is a muted word on the group and
// on its call, never a red box, and the call's excerpt waits behind a click.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { groupToolCalls, isToolCallGroup, type ToolCallGroup } from "../../../lib/toolCallGroups";
import { ToolCallGroupRow } from "./ToolCallGroupRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function tool(id: string, fields: Record<string, unknown>): AgentChatItem {
  return {
    id,
    role: "tool",
    title: "",
    output: "",
    toolCallId: id,
    status: "completed",
    ...fields,
  } as AgentChatItem;
}

/** The usual turn: read, grep, edit, then the tests fail. */
function turn(): ToolCallGroup {
  const grouped = groupToolCalls([
    tool("read", {
      kind: "read",
      title: "src/summary.ts",
      locations: [{ path: "src/summary.ts" }],
    }),
    tool("grep", { kind: "search", title: "buildHandoffSummary", output: "src/summary.ts:12" }),
    tool("edit", {
      kind: "edit",
      title: "src/summary.ts",
      locations: [{ path: "src/summary.ts" }],
      output: "- a\n+ b",
    }),
    tool("test", {
      kind: "execute",
      title: "pnpm test",
      command: "pnpm test",
      status: "failed",
      exitCode: 1,
      output:
        "> vitest run\n\n FAIL  summary.test.ts\nAssertionError: expected 1 to be 2\n  at summary.ts:58",
    }),
  ]);
  const group = grouped[0];
  if (group === undefined || !isToolCallGroup(group)) throw new Error("the turn did not group");
  return group;
}

let root: ReturnType<typeof createRoot> | null = null;
let host: HTMLDivElement;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  host.remove();
});

async function renderGroup(group: ToolCallGroup): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(<ToolCallGroupRow group={group} transcriptEnded={false} />));
  return host;
}

/** The person opens a line: the disclosure's own toggle, as the browser fires it. */
async function openLine(details: HTMLDetailsElement): Promise<void> {
  await act(async () => {
    details.open = true;
    details.dispatchEvent(new Event("toggle"));
  });
}

function groupOf(container: HTMLElement): HTMLDetailsElement {
  const group = container.querySelector<HTMLDetailsElement>("details.workspace-chat-tool-group");
  if (group === null) throw new Error("the group did not render");
  return group;
}

describe("a group that holds a failure", () => {
  it("renders one closed summary with a muted failure count and no call count or cross", async () => {
    const container = await renderGroup(turn());
    const group = groupOf(container);
    expect(group.open).toBe(false);
    expect(container.textContent).not.toContain("tool calls");
    expect(container.textContent).not.toContain("✗");
    expect(group.querySelector(".workspace-chat-tool-group-failed")?.textContent).toBe("1 failed");
  });

  it("shows no failure count when no call failed", async () => {
    const grouped = groupToolCalls([
      tool("a", { kind: "read", title: "a.ts", locations: [{ path: "a.ts" }] }),
      tool("b", { kind: "read", title: "b.ts", locations: [{ path: "b.ts" }] }),
    ]);
    const group = grouped[0];
    if (group === undefined || !isToolCallGroup(group)) throw new Error("no group");
    const container = await renderGroup(group);
    expect(container.querySelector(".workspace-chat-tool-group-failed")).toBeNull();
    expect(container.querySelector(".workspace-chat-tool-group-failures")).toBeNull();
  });

  it("keeps the failed call inside the group body, never outside it", async () => {
    const container = await renderGroup(turn());
    const group = groupOf(container);
    const summary = group.querySelector("summary");
    if (summary === null) throw new Error("the group has no summary");
    await act(async () => summary.click());
    expect(container.querySelector(".workspace-chat-tool-group-failures")).toBeNull();
    expect(group.querySelectorAll(".workspace-chat-tool")).toHaveLength(4);
  });

  it("shows the failed call as a muted word, and its excerpt only after the call is clicked", async () => {
    const container = await renderGroup(turn());
    const group = groupOf(container);
    await act(async () => group.querySelector("summary")?.click());
    const failedRow = container.querySelector<HTMLElement>(
      ".workspace-chat-tool-group-body .workspace-chat-tool.is-failed",
    );
    if (failedRow === null) throw new Error("the failed call did not render");
    const failedWord = failedRow.querySelector(".workspace-chat-tool-failed");
    expect(failedWord?.textContent).toBe("failed");
    expect(container.querySelector(".workspace-chat-tool-output.is-failure")).toBeNull();

    const details = failedRow.querySelector<HTMLDetailsElement>("details");
    if (details === null) throw new Error("the failed call has no line");
    await openLine(details);
    const excerpt = Array.from(
      failedRow.querySelectorAll(
        ".workspace-chat-tool-output.is-failure .workspace-chat-tool-output-line",
      ),
    ).map((line) => line.textContent);
    expect(excerpt).toEqual([
      " FAIL  summary.test.ts",
      "AssertionError: expected 1 to be 2",
      "  at summary.ts:58",
    ]);
  });

  it("keeps a failure's expanded output when the group opens and closes", async () => {
    const container = await renderGroup(turn());
    const group = groupOf(container);
    const summary = group.querySelector("summary");
    if (summary === null) throw new Error("the group has no summary");
    await act(async () => summary.click());
    const details = container.querySelector<HTMLDetailsElement>(
      ".workspace-chat-tool-group-body .workspace-chat-tool.is-failed details",
    );
    if (details === null) throw new Error("the failed call has no line");
    await openLine(details);
    const failureLines = () =>
      container.querySelectorAll(
        ".workspace-chat-tool-output.is-failure .workspace-chat-tool-output-line",
      );
    const expand = container.querySelector<HTMLButtonElement>(
      ".workspace-chat-tool-output.is-failure .workspace-chat-tool-more",
    );
    if (expand === null) throw new Error("the failure excerpt has no expander");
    expect(failureLines()).toHaveLength(3);
    await act(async () => expand.click());
    expect(failureLines()).toHaveLength(5);

    await act(async () => summary.click());
    expect(group.open).toBe(false);
    await act(async () => summary.click());
    expect(group.open).toBe(true);
    expect(failureLines()).toHaveLength(5);
  });
});
