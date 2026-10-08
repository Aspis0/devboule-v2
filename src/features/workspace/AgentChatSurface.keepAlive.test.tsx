// @vitest-environment happy-dom
import { act, StrictMode, useCallback, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AgentSession } from "../../lib/agentSession";
import { resetRegistry, syncOpenTabs } from "../../lib/agentSessionRegistry";
import type { SessionTask } from "../../types/ipc";
import { channelHarness } from "./sessionChannelHarness";

vi.mock("../../lib/tauri", async () => (await import("./sessionChannelHarness")).tauriMock);

import { sessionAttach, sessionDetach } from "../../lib/tauri";
import { AgentChatSurface } from "./AgentChatSurface";
import { useTasksAttention } from "./useTasksAttention";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  channelHarness.nextSubscriptionId = 41;
});
afterEach(async () => {
  await act(async () => root.unmount());
  resetRegistry();
  container.remove();
  vi.clearAllMocks();
});

function Pane({ visible = false }: { visible?: boolean }) {
  const [agent, setAgent] = useState<AgentSession | null>(null);
  const report = useCallback((_id: string, next: AgentSession | null) => setAgent(next), []);
  const news = useTasksAttention(
    agent === null ? null : { sessionId: "kept", source: agent },
    visible,
  );
  return (
    <>
      <span data-testid="news">{String(news)}</span>
      <AgentChatSurface
        daemonState="connected"
        sessionId="kept"
        onAgentChange={report}
        queueSupported
      />
    </>
  );
}

it("lights the Tasks dot on return after a hidden completion, and acknowledges it once", async () => {
  await act(async () =>
    root.render(
      <StrictMode>
        <Pane />
      </StrictMode>,
    ),
  );
  const emit = channelHarness.active;
  const task: SessionTask = {
    id: "child",
    kind: "agent",
    title: "Child job",
    state: "running",
    sessionId: "kept",
    childSessionId: "child",
    startedAtMs: 1,
  };
  await act(async () =>
    emit?.({ type: "tasks_snapshot", epoch: "e", revision: 1, tasks: [task], omitted: 0 }),
  );
  await act(async () => root.render(null));
  await act(async () =>
    emit?.({
      type: "tasks_snapshot",
      epoch: "e",
      revision: 2,
      tasks: [{ ...task, state: "finished" }],
      omitted: 0,
    }),
  );
  await act(async () =>
    root.render(
      <StrictMode>
        <Pane />
      </StrictMode>,
    ),
  );
  expect(container.querySelector('[data-testid="news"]')?.textContent).toBe("true");
  expect(container.textContent).toContain("Child job");
  expect(sessionAttach).toHaveBeenCalledTimes(1);
  expect(sessionDetach).not.toHaveBeenCalled();
  await act(async () =>
    root.render(
      <StrictMode>
        <Pane visible />
      </StrictMode>,
    ),
  );
  await act(async () =>
    root.render(
      <StrictMode>
        <Pane />
      </StrictMode>,
    ),
  );
  expect(container.querySelector('[data-testid="news"]')?.textContent).toBe("false");
});

it("restores the latest queue on remount and detaches only when the tab closes", async () => {
  await act(async () => root.render(<Pane />));
  const emit = channelHarness.active;
  await act(async () => root.render(null));
  await act(async () =>
    emit?.({
      type: "queue_snapshot",
      epoch: "e",
      revision: 1,
      items: [{ itemId: "next", text: "Queued while away" }],
    }),
  );
  await act(async () => root.render(<Pane />));
  expect(container.textContent).toContain("Queued while away");
  expect(sessionAttach).toHaveBeenCalledTimes(1);
  await act(async () => root.render(null));
  expect(sessionDetach).not.toHaveBeenCalled();
  syncOpenTabs([]);
  expect(sessionDetach).toHaveBeenCalledExactlyOnceWith(41);
});

it("uses the latest permission callbacks without recreating the session", async () => {
  const first = vi.fn();
  const latest = vi.fn();
  await act(async () =>
    root.render(
      <AgentChatSurface daemonState="connected" sessionId="kept" onPermissionRequest={first} />,
    ),
  );
  await act(async () =>
    root.render(
      <AgentChatSurface daemonState="connected" sessionId="kept" onPermissionRequest={latest} />,
    ),
  );
  await act(async () =>
    channelHarness.active?.({
      type: "permission_request",
      toolCallId: "tool",
      title: "Approve",
      options: [],
    }),
  );
  expect(first).not.toHaveBeenCalled();
  expect(latest).toHaveBeenCalledWith("kept", 41, expect.objectContaining({ toolCallId: "tool" }));
  expect(sessionAttach).toHaveBeenCalledTimes(1);
  expect(sessionDetach).not.toHaveBeenCalled();
});
