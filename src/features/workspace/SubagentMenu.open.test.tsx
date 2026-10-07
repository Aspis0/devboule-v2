// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AgentChatSurface } from "./AgentChatSurface";
import { channelHarness } from "./sessionChannelHarness";

vi.mock("../../lib/tauri", async () => (await import("./sessionChannelHarness")).tauriMock);
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  channelHarness.emit = null;
  channelHarness.active = null;
  channelHarness.activeSubscriptionId = null;
});

describe("a provider task row in the subagent menu", () => {
  it("stays unavailable when a roster session has its id, and takes no approval label from it", async () => {
    const open = vi.fn();
    const sameId = {
      id: "provider-internal",
      kind: "acp" as const,
      title: "Unrelated session",
      createdBy: "someone-else",
      state: { type: "live" as const, generation: 1 },
    };
    await act(async () =>
      root.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId="parent"
          onOpenSubagent={open}
          sessionRoster={[sameId]}
          subagentAttention={new Map([["provider-internal", "Needs your approval"]])}
        />,
      ),
    );
    await act(async () =>
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "provider-internal",
        title: "Internal task",
      }),
    );
    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    expect(pill?.getAttribute("aria-label")).toBe("Subagents: 1 working");
    await act(async () => pill?.click());

    const row = document.querySelector<HTMLButtonElement>("button.workspace-subagent-row");
    expect(row?.disabled).toBe(true);
    expect(row?.getAttribute("aria-label")).toBe("Internal task, running, Session unavailable");
    expect(row?.textContent).toContain("Unavailable");
    await act(async () => row?.click());
    expect(open).not.toHaveBeenCalled();
  });

  it("opens a created child's session from its row and closes the menu", async () => {
    const open = vi.fn();
    const child = {
      id: "child",
      kind: "acp" as const,
      title: "Child",
      createdBy: "parent",
      state: { type: "live" as const, generation: 1 },
    };
    await act(async () =>
      root.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId="parent"
          onOpenSubagent={open}
          sessionRoster={[child]}
        />,
      ),
    );
    await act(async () =>
      container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]')?.click(),
    );
    const row = document.querySelector<HTMLButtonElement>("button.workspace-subagent-row");
    if (row === null) throw new Error("child button did not render");
    row.focus();
    expect(row.getAttribute("type")).toBe("button");
    expect(row.textContent).toContain("Child");
    await act(async () => {
      row.click();
    });
    expect(open).toHaveBeenCalledExactlyOnceWith("child");
    expect(document.querySelector(".workspace-subagent-list")).toBeNull();
  });
});
