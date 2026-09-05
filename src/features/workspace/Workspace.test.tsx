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

vi.mock("./AgentChatSurface", () => ({
  AgentChatSurface: ({
    sessionId,
    onPermissionRequest,
    onPermissionResolved,
  }: {
    sessionId: string;
    onPermissionRequest?: (sessionId: string, request: PermissionRequest) => void;
    onPermissionResolved?: (sessionId: string, toolCallId: string) => void;
  }) => (
    <div data-testid="agent-chat-surface">
      {sessionId}
      <button
        type="button"
        data-testid="emit-permission-a"
        onClick={() =>
          onPermissionRequest?.(sessionId, {
            type: "permission_request",
            toolCallId: "tool-a",
            title: "Run command",
            command: "cmd.exe",
            args: ["/c", "echo", "alpha"],
            cwd: "C:\\alpha",
            options: [
              { optionId: "allow", name: "Allow once", kind: "allow_once" },
              { optionId: "deny", name: "Deny", kind: "reject_once" },
            ],
          })
        }
      />
      <button
        type="button"
        data-testid="emit-permission-b"
        onClick={() =>
          onPermissionRequest?.(sessionId, {
            type: "permission_request",
            toolCallId: "tool-b",
            title: "Run command",
            command: "ping.exe",
            args: ["-n", "1", "127.0.0.1"],
            cwd: "C:\\beta",
            options: [
              { optionId: "allow", name: "Allow once", kind: "allow_once" },
              { optionId: "deny", name: "Deny", kind: "reject_once" },
            ],
          })
        }
      />
      <button
        type="button"
        data-testid="emit-permission-resolved"
        onClick={() => onPermissionResolved?.(sessionId, "tool-a")}
      />
      <button
        type="button"
        data-testid="emit-permission-shared"
        onClick={() =>
          onPermissionRequest?.(sessionId, {
            type: "permission_request",
            toolCallId: "shared-tool",
            title: "Run command",
            command: `shared-${sessionId}`,
            cwd: `C:\\${sessionId}`,
            options: [
              { optionId: "allow", name: "Allow once", kind: "allow_once" },
              { optionId: "deny", name: "Deny", kind: "reject_once" },
            ],
          })
        }
      />
    </div>
  ),
}));

import { sessionCreate, sessionPermissionRespond, sessionsList } from "../../lib/tauri";
import { useAppStore } from "../../store/appStore";
import { Workspace, WorkspacePermissionCard } from "./Workspace";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const terminal = (id: string, title: string): Session => ({
  id,
  workspaceId: null,
  kind: "terminal",
  title,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
});

const acpSession = (id: string, title: string): Session => ({
  ...terminal(id, title),
  kind: "acp",
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

const spawnPermissionRequest: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-spawn",
  title: "Run command",
  command: "cmd.exe",
  args: ["/c", "echo", "gated"],
  cwd: "C:\\work\\tree",
  options: [
    { optionId: "allow", name: "Allow once", kind: "allow_once" },
    { optionId: "deny", name: "Deny", kind: "reject_once" },
  ],
};

describe("Workspace sessions", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    useAppStore.setState({ installedSkills: [] });
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(sessionsList).mockResolvedValue([terminal("session-1", "shell one")]);
    vi.mocked(sessionCreate).mockResolvedValue({
      ...terminal("session-2", "agent two"),
      kind: "acp",
    });
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it("renders real session tabs, creates an ACP session, and never renders the permission card", async () => {
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

    expect(sessionCreate).toHaveBeenCalledWith(null, "acp");
    expect(container.textContent).toContain("agent two");
    expect(container.querySelector("[data-testid=agent-chat-surface]")?.textContent).toBe(
      "session-2",
    );
  });

  it("starts an ACP session when a new workspace is added", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());

    expect(sessionCreate).toHaveBeenCalledWith(null, "acp");
    expect(container.querySelector("[data-testid=agent-chat-surface]")).not.toBeNull();
  });

  it("shows an empty Skills section when no skills are installed", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });

    expect(container.textContent).toContain("Skills");
    expect(container.textContent).toContain("No skills yet.");
    expect(container.querySelector(".workspace-skills-note")).toBeNull();
  });

  it("shows an installed skill in the Skills section", async () => {
    useAppStore.setState({
      installedSkills: [
        {
          id: "repo-rhythm",
          name: "Repo Rhythm",
          author: "@lena-code",
          description: "Turn a repository snapshot into a clear working plan.",
        },
      ],
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });

    expect(container.textContent).toContain("Skills");
    expect(container.textContent).toContain("Repo Rhythm");
    expect(container.querySelector(".workspace-skills-note")?.textContent).toBe(
      "Session only — not saved to disk.",
    );
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

  it("shows the command, args, and cwd that will be spawned", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          request={spawnPermissionRequest}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    const text = container.textContent ?? "";
    expect(text).toContain("cmd.exe");
    expect(text).toContain("/c");
    expect(text).toContain("echo");
    expect(text).toContain("gated");
    expect(text).toContain("C:\\work\\tree");
  });

  it("shows permission requests FIFO and advances after a response", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => undefined);

    const emitA = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    const emitB = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-b]");
    if (emitA === null || emitB === null) throw new Error("permission emitters did not render");
    await act(async () => emitA.click());
    await act(async () => emitB.click());

    const card = container.querySelector(".workspace-permission-card");
    if (card === null) throw new Error("permission card did not render");
    expect(card.textContent).toContain("cmd.exe");
    expect(card.textContent).toContain("alpha");
    expect(card.textContent).not.toContain("ping.exe");

    const allow = container.querySelector<HTMLButtonElement>(".workspace-primary-action");
    if (allow === null) throw new Error("permission allow control did not render");
    await act(async () => allow.click());
    await act(async () => undefined);

    const next = container.querySelector(".workspace-permission-card");
    if (next === null) throw new Error("second permission card did not render");
    expect(next.textContent).toContain("ping.exe");
    expect(next.textContent).toContain("C:\\beta");
    expect(sessionPermissionRespond).toHaveBeenCalledWith("session-2", "tool-a", "allow_once");
  });

  it("quotes args that contain spaces so they are not split visually", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          request={{
            ...spawnPermissionRequest,
            args: ["hello world"],
          }}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    const command = container.querySelector(".workspace-permission-command")?.textContent ?? "";
    expect(command).toContain('"hello world"');
    expect(command).not.toBe("cmd.exe hello world");
  });

  it("shows the selected session's permission when another session's request is at the head", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      acpSession("session-a", "agent a"),
      acpSession("session-b", "agent b"),
    ]);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const emitA = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    if (emitA === null) throw new Error("permission emitter A did not render");
    await act(async () => emitA.click());
    expect(container.querySelector(".workspace-permission-card")?.textContent).toContain("alpha");

    const tabB = [...container.querySelectorAll<HTMLButtonElement>(".workspace-session-tab")].find(
      (tab) => tab.textContent?.includes("agent b"),
    );
    if (tabB === undefined) throw new Error("session B tab did not render");
    await act(async () => tabB.click());
    await act(async () => undefined);

    const emitB = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-b]");
    if (emitB === null) throw new Error("permission emitter B did not render");
    await act(async () => emitB.click());

    const card = container.querySelector(".workspace-permission-card");
    if (card === null) throw new Error("selected session B's permission card did not render");
    expect(card.textContent).toContain("ping.exe");
    expect(card.textContent).not.toContain("alpha");
  });

  it("shows the full command including a long suffix the user must see before allowing", async () => {
    const suffix = "& del secrets.txt";
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          request={{
            ...spawnPermissionRequest,
            command: `${"echo ".padEnd(2100, "x")}${suffix}`,
            args: undefined,
          }}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    expect(container.textContent).toContain(suffix);
    expect(container.textContent).not.toContain("…");
  });

  it("lists env name=value on the permission card when present", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspacePermissionCard
          sessionId="session-1"
          request={{
            ...spawnPermissionRequest,
            env: [{ name: "DB_GATE", value: "SAFE & echo PWNED" }],
          }}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    const env = container.querySelector(".workspace-permission-env")?.textContent ?? "";
    expect(env).toContain("DB_GATE=SAFE & echo PWNED");
  });

  it("drops the card when the backend resolves the permission without a UI click", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => undefined);

    const emit = container.querySelector<HTMLButtonElement>("[data-testid=emit-permission-a]");
    const resolved = container.querySelector<HTMLButtonElement>(
      "[data-testid=emit-permission-resolved]",
    );
    if (emit === null || resolved === null) throw new Error("permission emitters did not render");
    await act(async () => emit.click());
    expect(container.querySelector(".workspace-permission-card")).not.toBeNull();

    await act(async () => resolved.click());
    expect(container.querySelector(".workspace-permission-card")).toBeNull();
    expect(sessionPermissionRespond).not.toHaveBeenCalled();
  });

  it("keeps the other session's card when two sessions share a toolCallId", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      acpSession("session-a", "agent a"),
      acpSession("session-b", "agent b"),
    ]);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const emitShared = container.querySelector<HTMLButtonElement>(
      "[data-testid=emit-permission-shared]",
    );
    if (emitShared === null) throw new Error("shared permission emitter did not render");
    await act(async () => emitShared.click());
    expect(container.querySelector(".workspace-permission-card")?.textContent).toContain(
      "shared-session-a",
    );

    const tabB = [...container.querySelectorAll<HTMLButtonElement>(".workspace-session-tab")].find(
      (tab) => tab.textContent?.includes("agent b"),
    );
    if (tabB === undefined) throw new Error("session B tab did not render");
    await act(async () => tabB.click());
    await act(async () => undefined);

    const emitB = container.querySelector<HTMLButtonElement>(
      "[data-testid=emit-permission-shared]",
    );
    if (emitB === null) throw new Error("session B shared emitter did not render");
    await act(async () => emitB.click());
    expect(container.querySelector(".workspace-permission-card")?.textContent).toContain(
      "shared-session-b",
    );

    const tabA = [...container.querySelectorAll<HTMLButtonElement>(".workspace-session-tab")].find(
      (tab) => tab.textContent?.includes("agent a"),
    );
    if (tabA === undefined) throw new Error("session A tab did not render");
    await act(async () => tabA.click());
    await act(async () => undefined);

    const allow = container.querySelector<HTMLButtonElement>(".workspace-primary-action");
    if (allow === null) throw new Error("permission allow control did not render");
    await act(async () => allow.click());
    await act(async () => undefined);

    expect(container.querySelector(".workspace-permission-card")).toBeNull();
    expect(sessionPermissionRespond).toHaveBeenCalledWith("session-a", "shared-tool", "allow_once");

    await act(async () => tabB.click());
    await act(async () => undefined);
    const cardB = container.querySelector(".workspace-permission-card");
    if (cardB === null) throw new Error("session B's card vanished after resolving A");
    expect(cardB.textContent).toContain("shared-session-b");
    expect(cardB.textContent).not.toContain("shared-session-a");
  });
});
