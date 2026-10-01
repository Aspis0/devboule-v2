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

describe("opening a child through the chat header", () => {
  it("refreshes missing daemon sessions once on open and disables an unresolved provider task", async () => {
    const open = vi.fn();
    const refresh = vi.fn(async () => undefined);
    await act(async () =>
      root.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId="parent"
          title="Parent"
          onOpenSubagent={open}
          subagentSessionIds={new Set()}
          onRefreshSubagents={refresh}
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
    await act(async () =>
      container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]')?.click(),
    );
    const row = document.querySelector<HTMLButtonElement>("button.workspace-subagent-row");
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(row?.disabled).toBe(true);
    expect(row?.getAttribute("aria-label")).toContain("running, Session unavailable");
    expect(row?.textContent).toContain("Unavailable");
    await act(async () => row?.click());
    expect(open).not.toHaveBeenCalled();
    expect(refresh).toHaveBeenCalledTimes(1);
    await act(async () =>
      root.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId="parent"
          title="Parent"
          onOpenSubagent={open}
          subagentSessionIds={new Set(["provider-internal"])}
          subagentAttention={new Map([["provider-internal", "Needs your approval"]])}
          onRefreshSubagents={refresh}
        />,
      ),
    );
    expect(row?.disabled).toBe(false);
    expect(row?.getAttribute("aria-label")).toContain("Needs your approval, Open in tab");
    expect(container.querySelector('[data-testid="subagent-pill"]')?.textContent).toContain(
      "1 needs your approval",
    );
    await act(async () => row?.click());
    expect(open).toHaveBeenCalledWith("provider-internal");
  });

  it("routes a child button click to its session ID", async () => {
    const open = vi.fn();
    await act(async () =>
      root.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId="parent"
          title="Parent"
          onOpenSubagent={open}
          subagentSessionIds={new Set(["child"])}
        />,
      ),
    );
    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "child",
        title: "Child",
        spawnDepth: 1,
      });
    });
    expect(open).not.toHaveBeenCalled();
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
