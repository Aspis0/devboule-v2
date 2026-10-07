// `/goal` in the composer's slash menu: one universal entry the surface
// appends for a live agent session — `/goal x` travels to the daemon as
// plain text, which intercepts it — deduped against a provider's own
// `goal` (Codex), and never offered to a dead row. Terminals never mount
// this surface at all: they render no composer and no slash menu.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionState } from "../../types/ipc";
import { channelHarness } from "./sessionChannelHarness";

vi.mock("../../lib/tauri", async () => (await import("./sessionChannelHarness")).tauriMock);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => undefined),
  Channel: class {
    onmessage: (event: unknown) => void;
    constructor(onmessage: (event: unknown) => void) {
      this.onmessage = onmessage;
    }
  },
}));

vi.mock("../terminal/createTerminalView", () => ({
  createTerminalView: async () => ({
    write: (_data: string, callback?: () => void) => callback?.(),
    applySnapshot: (_snapshot: unknown, callback: () => void) => callback(),
    fit: () => true,
    dispose: () => undefined,
    cols: () => 80,
    rows: () => 24,
  }),
}));

import { AgentChatSurface, goalCommandsFor, withGoalCommand } from "./AgentChatSurface";
import { RECOVERED } from "./sessionStateFixtures";
import { TerminalSurface } from "../terminal/TerminalSurface";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

const keyFor = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const UNIVERSAL_DESCRIPTION = "Set, show, or clear this session's goal";
const CODEX_DESCRIPTION = "Set, pause, resume, or clear the agent's goal";
const ENDED: SessionState = {
  type: "ended",
  generation: 1,
  code: 1,
  integrity: { kind: "complete" },
};

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

function typeSlash(): void {
  const textarea = container.querySelector<HTMLTextAreaElement>(
    'textarea[aria-label="Message the agent"]',
  );
  if (textarea === null) throw new Error("agent chat composer did not render");
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  if (setValue === undefined) throw new Error("textarea value setter did not exist");
  setValue.call(textarea, "/");
  textarea.dispatchEvent(new Event("input", { bubbles: true }));
}

function menuOptions(): Element[] {
  return [...container.querySelectorAll(".workspace-command-option")];
}

async function renderAgent(observedState: SessionState | null = null): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root?.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId="agent-1"
        observedState={observedState}
      />,
    );
  });
  await act(async () => undefined);
}

describe("the universal /goal entry", () => {
  it("appends exactly one entry with the universal description", () => {
    const merged = withGoalCommand(
      [{ name: "compact", description: "Compress conversation history" }],
      true,
    );

    expect(merged).toHaveLength(2);
    expect(merged[1]).toEqual({ name: "goal", description: UNIVERSAL_DESCRIPTION });
  });

  it("keeps the provider's entry and description on a name clash", () => {
    const merged = withGoalCommand(
      [
        { name: "compact", description: "Compress conversation history" },
        { name: "goal", description: CODEX_DESCRIPTION },
      ],
      true,
    );

    expect(merged.filter((command) => command.name === "goal")).toHaveLength(1);
    expect(merged).toContainEqual({ name: "goal", description: CODEX_DESCRIPTION });
  });

  it("dedupes case-insensitively", () => {
    const merged = withGoalCommand([{ name: "Goal", description: CODEX_DESCRIPTION }], true);

    expect(merged).toHaveLength(1);
  });

  it("adds nothing when the session is not live", () => {
    const commands = [{ name: "compact", description: "Compress conversation history" }];

    expect(withGoalCommand(commands, false)).toEqual(commands);
    expect(withGoalCommand([], false)).toEqual([]);
  });
});

describe("the /goal gate on the built command list", () => {
  it("offers /goal for a null observed state (defensive arm)", () => {
    const merged = goalCommandsFor(
      [{ name: "compact", description: "Compress conversation history" }],
      null,
    );

    expect(merged.map((command) => command.name)).toContain("goal");
  });

  it("offers /goal for live and silent sessions", () => {
    for (const observedState of [
      { type: "live", generation: 1 },
      { type: "silent", generation: 1 },
    ] as const) {
      const merged = goalCommandsFor([], observedState);

      expect(merged.map((command) => command.name)).toContain("goal");
    }
  });

  it("offers no /goal for ended or recovered sessions", () => {
    for (const observedState of [ENDED, RECOVERED]) {
      expect(goalCommandsFor([], observedState)).toEqual([]);
      expect(
        goalCommandsFor(
          [{ name: "compact", description: "Compress conversation history" }],
          observedState,
        ).map((command) => command.name),
      ).not.toContain("goal");
    }
  });
});

describe("the /goal entry on the surface", () => {
  it("appears for a running agent session with an empty catalog", async () => {
    await renderAgent();
    typeSlash();

    const options = menuOptions();
    expect(options).toHaveLength(1);
    expect(options[0]?.textContent).toContain("/goal");
    expect(options[0]?.textContent).toContain(UNIVERSAL_DESCRIPTION);
  });

  it("is absent for an ended session", async () => {
    await renderAgent(ENDED);
    typeSlash();

    expect(container.querySelector('[aria-label="Available commands"]')).toBeNull();
    expect(menuOptions()).toHaveLength(0);
  });

  it("keeps the provider's description on a Codex session", async () => {
    await renderAgent();
    await act(async () => {
      channelHarness.active?.({
        type: "available_commands",
        commands: [
          { name: "compact", description: "Compress conversation history" },
          { name: "goal", description: CODEX_DESCRIPTION },
        ],
      });
    });
    typeSlash();

    const goals = menuOptions().filter((option) => option.textContent?.includes("/goal"));
    expect(goals).toHaveLength(1);
    expect(goals[0]?.textContent).toContain(CODEX_DESCRIPTION);
    expect(goals[0]?.textContent).not.toContain(UNIVERSAL_DESCRIPTION);
  });
});

describe("the /goal entry and terminals", () => {
  it("renders no composer and no slash menu on a terminal", async () => {
    root = createRoot(container);
    await act(async () => {
      root?.render(<TerminalSurface workspaceKey={keyFor("w1")} sessionId="session-1" />);
    });
    await act(async () => undefined);

    expect(container.querySelector(".workspace-composer-track")).toBeNull();
    expect(container.querySelector('textarea[aria-label="Message the agent"]')).toBeNull();
    expect(container.querySelector('[aria-label="Available commands"]')).toBeNull();
  });
});
