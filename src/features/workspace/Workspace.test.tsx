// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Session } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  daemonStatus: vi.fn(async () => ({
    state: "connected",
    pid: 42,
    instanceId: "daemon-test",
    protocolVersion: 1,
    clients: 1,
    message: null,
  })),
  sessionsList: vi.fn(),
  sessionCreate: vi.fn(),
  createSessionStateChannel: vi.fn((onSnapshot: (snapshots: unknown[]) => void) => ({
    onSnapshot,
  })),
  sessionsWatch: vi.fn(async () => undefined),
  sessionsUnwatch: vi.fn(async () => undefined),
}));

vi.mock("../terminal/TerminalSurface", () => ({
  TerminalSurface: ({ sessionId }: { sessionId: string }) => (
    <div data-testid="terminal-surface">{sessionId}</div>
  ),
}));

import { sessionCreate, sessionsList } from "../../lib/tauri";
import { Workspace } from "./Workspace";

const terminal = (id: string, title: string): Session => ({
  id,
  workspaceId: null,
  kind: "terminal",
  title,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
});

describe("Workspace terminal sessions", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(sessionsList).mockResolvedValue([terminal("session-1", "shell one")]);
    vi.mocked(sessionCreate).mockResolvedValue(terminal("session-2", "shell two"));
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it("renders real terminal tabs, creates through session_create, and never renders the permission card", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    expect(container.textContent).toContain("shell one");
    expect(container.querySelector(".workspace-permission-card")).toBeNull();
    expect(container.querySelector("[data-testid=terminal-surface]")?.textContent).toBe(
      "session-1",
    );

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());

    expect(sessionCreate).toHaveBeenCalledWith(null, "terminal");
    expect(container.textContent).toContain("shell two");
  });
});
