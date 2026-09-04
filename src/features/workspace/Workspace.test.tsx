// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PermissionRequest, Session } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  daemonStatus: vi.fn(async () => ({
    state: "connected",
    pid: 42,
    instanceId: "daemon-test",
    protocolVersion: 1,
    clients: 1,
    capabilities: ["typed_permissions"],
    message: null,
  })),
  sessionsList: vi.fn(),
  sessionCreate: vi.fn(),
  sessionPermissionRespond: vi.fn(async () => undefined),
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

import { sessionCreate, sessionPermissionRespond, sessionsList } from "../../lib/tauri";
import { Workspace, WorkspacePermissionCard } from "./Workspace";

const terminal = (id: string, title: string): Session => ({
  id,
  workspaceId: null,
  kind: "terminal",
  title,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
});

const permissionRequest: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-test",
  title: "Run command",
  command: "echo test",
  options: [
    { optionId: "allow", name: "Allow once", kind: "allow_once" },
    { optionId: "deny", name: "Deny", kind: "reject_once" },
  ],
};

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

  it("does not render a permission card before typed_permissions is negotiated", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          request={permissionRequest}
          capabilities={[]}
        />,
      );
    });

    expect(container.querySelector(".workspace-permission-card")).toBeNull();
    expect(sessionPermissionRespond).not.toHaveBeenCalled();
  });

  it("sends one real allow-once response for a negotiated permission card", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          request={permissionRequest}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    const allow = container.querySelector<HTMLButtonElement>(".workspace-primary-action");
    if (allow === null) throw new Error("permission allow control did not render");
    await act(async () => {
      allow.click();
      allow.click();
    });

    expect(sessionPermissionRespond).toHaveBeenCalledTimes(1);
    expect(sessionPermissionRespond).toHaveBeenCalledWith("session-1", "tool-test", "allow_once");
  });
});
