// The shipped /goal gate: the commands the composer receives for ended,
// recovered and live sessions.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionState } from "../../types/ipc";
import { channelHarness } from "./sessionChannelHarness";
import { RECOVERED } from "./sessionStateFixtures";

vi.mock("../../lib/tauri", async () => (await import("./sessionChannelHarness")).tauriMock);

vi.mock("./WorkspaceComposer", () => ({
  WorkspaceComposer: ({
    availableCommands = [],
  }: {
    availableCommands?: readonly { name: string }[];
  }) => (
    <div
      data-testid="composer-capture"
      data-commands={availableCommands.map((command) => command.name).join(",")}
    />
  ),
}));

import { AgentChatSurface } from "./AgentChatSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const ENDED: SessionState = {
  type: "ended",
  generation: 1,
  code: 1,
  integrity: { kind: "complete" },
};
const LIVE: SessionState = { type: "live", generation: 1 };

let container: HTMLDivElement;
let root: Root | null;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = null;
  channelHarness.emit = null;
  channelHarness.active = null;
  channelHarness.activeSubscriptionId = null;
  channelHarness.nextSubscriptionId = 41;
  channelHarness.deferNextAttach = false;
  channelHarness.releaseNextAttach = null;
});

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

function commands(): string {
  return (
    container.querySelector('[data-testid="composer-capture"]')?.getAttribute("data-commands") ?? ""
  );
}

async function renderSurface(observedState: SessionState | null): Promise<void> {
  root = createRoot(container);
  await rerenderSurface(observedState);
}

async function rerenderSurface(observedState: SessionState | null): Promise<void> {
  await act(async () => {
    root?.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId="agent-1"
        title="Agent"
        observedState={observedState}
      />,
    );
  });
  await act(async () => undefined);
}

describe("the shipped /goal gate", () => {
  it("hands the composer no goal entry for an ended session", async () => {
    await renderSurface(ENDED);

    expect(commands().split(",").filter(Boolean)).not.toContain("goal");
  });

  it("hands the composer no goal entry for a recovered session", async () => {
    await renderSurface(RECOVERED);

    expect(commands().split(",").filter(Boolean)).not.toContain("goal");
  });

  it("hands the composer a goal entry for a live session", async () => {
    await renderSurface(LIVE);

    expect(commands().split(",").filter(Boolean)).toContain("goal");
  });

  it("drops the goal entry when the same surface goes live to ended", async () => {
    await renderSurface(LIVE);
    expect(commands().split(",").filter(Boolean)).toContain("goal");

    await rerenderSurface(ENDED);

    expect(commands().split(",").filter(Boolean)).not.toContain("goal");
  });

  it("drops the goal entry when the same surface goes live to recovered", async () => {
    await renderSurface(LIVE);
    expect(commands().split(",").filter(Boolean)).toContain("goal");

    await rerenderSurface(RECOVERED);

    expect(commands().split(",").filter(Boolean)).not.toContain("goal");
  });
});
