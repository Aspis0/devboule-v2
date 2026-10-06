// @vitest-environment happy-dom

// A failure is never behind a click: a group that holds one shows that call,
// line and excerpt, under the group line while the rest stays collapsed.

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

describe("a group that holds a failure", () => {
  it("stands the failed call's line and excerpt under the closed group line", async () => {
    const container = await renderGroup(turn());
    const group = container.querySelector<HTMLDetailsElement>("details.workspace-chat-tool-group");
    if (group === null) throw new Error("the group did not render");
    expect(group.open).toBe(false);

    const failures = container.querySelector(".workspace-chat-tool-group-failures");
    if (failures === null) throw new Error("the failure is behind the group's click");
    expect(group.contains(failures)).toBe(false);
    // Only the failed call stands outside: the others stay in the closed body.
    expect(failures.querySelectorAll(".workspace-chat-tool")).toHaveLength(1);
    expect(failures.querySelector(".workspace-command-chip")?.textContent).toBe("pnpm test");
    expect(failures.querySelector(".workspace-chat-tool-failed")?.textContent).toContain("failed");
    const excerpt = Array.from(
      failures.querySelectorAll(
        ".workspace-chat-tool-output.is-failure .workspace-chat-tool-output-line",
      ),
    ).map((line) => line.textContent);
    expect(excerpt).toEqual([
      " FAIL  summary.test.ts",
      "AssertionError: expected 1 to be 2",
      "  at summary.ts:58",
    ]);
    // Mounted once: the failure is outside the group while it is closed, not also in its body.
    expect(group.querySelectorAll(".workspace-chat-tool")).toHaveLength(3);
    expect(
      group.querySelector(".workspace-chat-tool-group-body .workspace-chat-tool-failed"),
    ).toBeNull();
  });

  it("does not show the failure twice once the group is open", async () => {
    const container = await renderGroup(turn());
    const group = container.querySelector<HTMLDetailsElement>("details.workspace-chat-tool-group");
    const summary = group?.querySelector("summary");
    if (group === null || summary === null || summary === undefined) {
      throw new Error("the group did not render");
    }
    await act(async () => summary.click());

    expect(group.open).toBe(true);
    expect(container.querySelector(".workspace-chat-tool-group-failures")).toBeNull();
    expect(container.querySelectorAll(".workspace-chat-tool-output.is-failure")).toHaveLength(1);
  });

  it("leaves a group with no failure as one closed line", async () => {
    const grouped = groupToolCalls([
      tool("a", { kind: "read", title: "a.ts" }),
      tool("b", { kind: "read", title: "b.ts" }),
    ]);
    const group = grouped[0];
    if (group === undefined || !isToolCallGroup(group)) throw new Error("no group");
    const container = await renderGroup(group);
    expect(container.querySelector(".workspace-chat-tool-group-failures")).toBeNull();
  });
});
