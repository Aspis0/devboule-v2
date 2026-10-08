// @vitest-environment happy-dom
import { act, useEffect, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type {
  PermissionRequest,
  PermissionResolved,
  Project,
  Session,
  SessionEvent,
  Workspace as IpcWorkspace,
} from "../../types/ipc";
import {
  acquire,
  peek,
  pendingPermissionRequests,
  release,
  resetRegistry,
} from "../../lib/agentSessionRegistry";
import type { AgentChannel, AgentSessionDeps } from "../../lib/agentSession";

const channel = vi.hoisted(() => ({ emit: null as ((event: SessionEvent) => void) | null }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: vi.fn(async () => false) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("../../lib/tauri", () => ({
  daemonStatus: vi.fn(async () => ({
    state: "connected",
    pid: 42,
    instanceId: "test",
    protocolVersion: 1,
    clients: 1,
    capabilities: ["typed_permissions"],
    message: null,
  })),
  projectsList: vi.fn(),
  workspacesList: vi.fn(),
  sessionsList: vi.fn(),
  providersList: vi.fn(),
  workspaceGitStatus: vi.fn(async () => ({
    isGit: true,
    dirty: false,
    branch: "main",
    totals: { additions: 0, deletions: 0 },
    rows: [],
    error: null,
  })),
  workspaceGitDiff: vi.fn(async () => null),
  workspaceFilesList: vi.fn(async () => ({
    path: "",
    entries: [],
    capped: false,
    skipped: 0,
    error: null,
  })),
  journalUsage: vi.fn(),
  sessionPresence: vi.fn(async () => undefined),
  sessionStop: vi.fn(async () => undefined),
  sessionClose: vi.fn(async () => undefined),
  sessionPermissionRespond: vi.fn(
    async (_id: string, _subscription: number, toolCallId: string) => {
      channel.emit?.({ type: "permission_resolved", toolCallId });
    },
  ),
  createSessionStateChannel: vi.fn(() => ({})),
  sessionsWatch: vi.fn(async () => undefined),
  sessionsUnwatch: vi.fn(async () => undefined),
  delegationGet: vi.fn(async () => ({ enabled: false, source: "default" })),
  delegationSet: vi.fn(async () => undefined),
  devicesList: vi.fn(async () => ({ selfInfo: undefined, peers: [], pending: [] })),
}));
vi.mock("./AgentChatSurface", () => ({
  AgentChatSurface: ({
    sessionId,
    auxiliary,
    onPermissionRequest,
    onPermissionResolved,
  }: {
    sessionId: string;
    auxiliary?: ReactNode;
    onPermissionRequest?: (id: string, subscription: number, request: PermissionRequest) => void;
    onPermissionResolved?: (id: string, resolution: PermissionResolved) => void;
  }) => {
    useEffect(() => {
      const entry = acquire(
        {
          sessionId,
          invoke: vi.fn(async (cmd) =>
            cmd === "session_attach" ? 41 : undefined,
          ) as AgentSessionDeps["invoke"],
          createChannel: (emit) => {
            channel.emit = emit;
            return {} as AgentChannel;
          },
          onPermissionRequest: (request, subscription) =>
            onPermissionRequest?.(sessionId, subscription, request),
          onPermissionResolved: (resolution) => onPermissionResolved?.(sessionId, resolution),
        },
        1,
      );
      return () => release(entry);
    }, [sessionId, onPermissionRequest, onPermissionResolved]);
    return <div>{auxiliary}</div>;
  },
}));

import {
  projectsList,
  workspacesList,
  sessionsList,
  providersList,
  sessionPermissionRespond,
} from "../../lib/tauri";
import { Workspace } from "./Workspace";
import { resetSharedSessionControllerForTests } from "./workspaceSessions";
import { openListedSessionsForTest } from "./workspaceSessionTestSetup";
import { resetTabMemoryForTests } from "./workspaceTabMemory";
import { resetSharedCloseActionsForTests } from "./strip/closeActions";
import { setLastSelectedWorkspaceKey } from "./lastSelectedWorkspace";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const project: Project = { id: "project", name: "devboule", path: "C:\\devboule" };
const workspace: IpcWorkspace = {
  id: "workspace",
  projectId: "project",
  title: "main",
  isolation: "local",
  path: "C:\\devboule",
};
const session: Session = {
  id: "permission-chat",
  workspaceId: "workspace",
  kind: "acp",
  title: "Chat",
  createdAtMs: 1,
  elapsedMs: 0,
  state: { type: "live", generation: 1 },
};
const request: PermissionRequest = {
  type: "permission_request",
  toolCallId: "approve-hidden",
  title: "Run command",
  command: "echo",
  options: [
    { optionId: "allow", name: "Allow once", kind: "allow_once" },
    { optionId: "deny", name: "Deny", kind: "reject_once" },
  ],
};
let container: HTMLDivElement;
let root: Root;
beforeEach(() => {
  resetSharedSessionControllerForTests();
  resetTabMemoryForTests();
  resetSharedCloseActionsForTests();
  setLastSelectedWorkspaceKey(null);
  window.localStorage.clear();
  vi.useFakeTimers();
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  vi.mocked(projectsList).mockResolvedValue([project]);
  vi.mocked(workspacesList).mockResolvedValue([workspace]);
  vi.mocked(sessionsList).mockResolvedValue([session]);
  vi.mocked(providersList).mockResolvedValue({ providers: [], unreadableDirs: 0 });
});
afterEach(async () => {
  await act(async () => root.unmount());
  resetRegistry();
  resetSharedSessionControllerForTests();
  resetSharedCloseActionsForTests();
  container.remove();
  vi.useRealTimers();
  vi.clearAllMocks();
  channel.emit = null;
});
async function mount() {
  await act(async () => {
    await openListedSessionsForTest();
    root.render(<Workspace />);
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
}
async function away() {
  await act(async () => root.render(null));
}
function cards() {
  return container.querySelectorAll(".permission-card");
}

it("restores an approval received while Workspace is unmounted and answers it once", async () => {
  await mount();
  await away();
  await act(async () => channel.emit?.(request));
  await mount();
  expect(cards()).toHaveLength(1);
  await act(async () => channel.emit?.(request));
  expect(cards()).toHaveLength(1);
  const allow = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
  expect(allow).not.toBeNull();
  await act(async () => allow?.click());
  expect(sessionPermissionRespond).toHaveBeenCalledExactlyOnceWith(
    "permission-chat",
    41,
    "approve-hidden",
    "allow_once",
    undefined,
    undefined,
  );
  await away();
  await mount();
  expect(cards()).toHaveLength(0);
});

it("does not restore a request resolved while Workspace is unmounted", async () => {
  await mount();
  await away();
  await act(async () => {
    channel.emit?.(request);
    channel.emit?.({ type: "permission_resolved", toolCallId: request.toolCallId });
  });
  await mount();
  expect(cards()).toHaveLength(0);
});

it("forgets a request refused as stale, so a remount never re-seeds it and the entry can be evicted", async () => {
  vi.mocked(sessionPermissionRespond).mockRejectedValueOnce({
    code: "invalid_request",
    message: "permission request is no longer pending",
  });
  await mount();
  await act(async () => channel.emit?.(request));
  const allow = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
  await act(async () => allow?.click());
  expect(container.querySelector(".permission-card-dismiss-action")).not.toBeNull();
  expect(pendingPermissionRequests()).toEqual([]);
  await away();
  await mount();
  expect(cards()).toHaveLength(0);
  await away();
  for (let i = 0; i < 8; i++) {
    const entry = acquire(
      {
        sessionId: `busy-${i}`,
        invoke: vi.fn(async (cmd) =>
          cmd === "session_attach" ? 41 : undefined,
        ) as AgentSessionDeps["invoke"],
        createChannel: () => ({}) as AgentChannel,
      },
      1,
    );
    await entry.session.start();
    await entry.session.send("work");
    release(entry);
  }
  expect(peek("permission-chat", 1)).toBeUndefined();
});
