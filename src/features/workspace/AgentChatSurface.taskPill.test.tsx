// The checklist pill in the composer track: an `agent_tasks` frame puts it
// on the track above the queued follow-ups, and the count it shows is the
// frame's.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { channelHarness } from "./sessionChannelHarness";

vi.mock("../../lib/tauri", async () => (await import("./sessionChannelHarness")).tauriMock);

import { AgentChatSurface } from "./AgentChatSurface";
import { createInMemoryMessageQueue } from "./inMemoryMessageQueue";
import { idleSender } from "./queueTestKit";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  channelHarness.emit = null;
  channelHarness.active = null;
  channelHarness.activeSubscriptionId = null;
  channelHarness.nextSubscriptionId = 41;
  channelHarness.deferNextAttach = false;
  channelHarness.releaseNextAttach = null;
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("the checklist pill in the composer track", () => {
  it("shows an agent_tasks frame above the queued follow-ups", async () => {
    const queue = createInMemoryMessageQueue("agent-1", idleSender());
    root = createRoot(container);
    await act(async () => {
      root.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId="agent-1"
          title="Agent"
          queue={queue}
        />,
      );
    });
    await act(async () => undefined);
    act(() => queue.add("a second look afterwards", []));

    const track = container.querySelector(".workspace-composer-track");
    if (track === null) throw new Error("no composer track");
    expect(track.querySelector('[data-testid="agent-task-pill"]')).toBeNull();

    await act(async () => {
      channelHarness.active?.({
        type: "agent_tasks",
        items: [
          { id: "t-1", text: "Read the journal", status: "completed" },
          { id: "t-2", text: "Replay the frames", status: "in_progress" },
          { id: "t-3", text: "Write the report", status: "pending" },
        ],
      });
    });

    const pill = track.querySelector('[data-testid="agent-task-pill"]');
    expect(pill).not.toBeNull();
    expect(track.textContent).toContain("1 of 3");
    // The frame's running item is the current step, never a "next:" one.
    expect(track.textContent).toContain("Replay the frames");
    expect(track.textContent).not.toContain("next:");
    const queueTrack = track.querySelector(".workspace-queue-track");
    expect(queueTrack).not.toBeNull();
    if (pill === null || queueTrack === null) throw new Error("pill or queue track missing");
    // The checklist is the first row of the track, the queue behind it.
    const children = [...track.children];
    expect(children.indexOf(pill)).toBeLessThan(children.indexOf(queueTrack));
  });
});
