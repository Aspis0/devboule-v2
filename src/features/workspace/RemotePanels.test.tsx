// @vitest-environment happy-dom

// The remote panels: a paired host's Files tree and Changes rows over
// the held peer link, read-only. Rows render from the relayed replies;
// clicking a file opens a tab on that host (never the local daemon);
// no row writes anywhere (no menus, no acts, no commits, no diffs).

import { act } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceDirectory, WorkspaceFileEntry, WorkspaceGitStatus } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  daemonStatus: vi.fn(),
  remoteHostFilesList: vi.fn(),
  remoteHostGitStatus: vi.fn(),
  remoteHostFileOpen: vi.fn(),
  remoteHostFileVersion: vi.fn(),
  remoteHostFileWrite: vi.fn(),
  workspaceFileRead: vi.fn(),
  workspaceFilePreviewStage: vi.fn(),
  workspaceFilePreviewUnstage: vi.fn(),
  editorTargetsList: vi.fn(async () => []),
}));

import {
  daemonStatus,
  remoteHostFilesList,
  remoteHostFileOpen,
  remoteHostFileWrite,
  remoteHostGitStatus,
} from "../../lib/tauri";
import { FilesSurface } from "./FilesSurface";
import { ChangesSurface } from "./ChangesSurface";
import { WorkspaceFileTab } from "./WorkspaceFileTab";
import { workspaceKey, type HostId, type WorkspaceKey } from "./hosts/hostIdentity";

const remoteKeyFor = (deviceId: string, workspaceId: string): WorkspaceKey =>
  workspaceKey(deviceId as HostId, workspaceId)!;

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const DEVICE = "device-9";
const WORKSPACE = "workspace-remote-subject";

function entry(path: string, kind: WorkspaceFileEntry["kind"]): WorkspaceFileEntry {
  const segments = path.split("/");
  return { path, name: segments[segments.length - 1]!, kind, size: null };
}

function listing(entries: WorkspaceFileEntry[]): WorkspaceDirectory {
  return { path: "", entries, capped: false, skipped: 0, error: null };
}

function status(): WorkspaceGitStatus {
  return {
    isGit: true,
    dirty: true,
    branch: "main",
    totals: { additions: 1, deletions: 0 },
    rows: [
      {
        path: "note.txt",
        status: "modified",
        additions: 1,
        deletions: 0,
        capped: false,
        renamedFrom: null,
      },
    ],
    error: null,
  };
}

describe("remote panels", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(daemonStatus).mockResolvedValue({
      state: "connected",
      pid: 1,
      instanceId: "1",
      protocolVersion: 34,
      clients: 1,
      capabilities: [],
      message: null,
    } as never);
    vi.mocked(remoteHostFilesList).mockResolvedValue(
      listing([entry("docs", "dir"), entry("note.txt", "file")]),
    );
    vi.mocked(remoteHostGitStatus).mockResolvedValue(status());
    vi.mocked(remoteHostFileOpen).mockResolvedValue({
      status: "ok",
      content: "hi\n",
      hasBom: false,
      version: {
        status: "ready",
        workspaceId: WORKSPACE,
        path: "note.txt",
        size: 3,
        modifiedAt: 100,
        revision: "3:100",
      },
      size: 3,
      error: null,
    });
    vi.mocked(remoteHostFileWrite).mockResolvedValue({
      status: "written",
      modifiedAt: 101,
      size: 3,
      revision: "3:101",
    });
  });

  afterEach(async () => {
    if (root !== undefined) {
      await act(async () => root!.unmount());
      root = undefined;
    }
    container.remove();
    vi.clearAllMocks();
  });

  async function render(node: ReactNode) {
    if (root === undefined) root = createRoot(container);
    await act(async () => {
      root!.render(node);
    });
  }

  it("lists the host's tree and asks the host, never the local daemon", async () => {
    await render(
      <FilesSurface workspaceKey={remoteKeyFor(DEVICE, WORKSPACE)} onOpenFile={() => undefined} />,
    );

    expect(remoteHostFilesList).toHaveBeenCalledWith(DEVICE, WORKSPACE, "");
    expect(container.textContent).toContain("note.txt");
    // Read-only: folders expand, but no row carries a menu.
    expect(container.querySelector(".workspace-tree-menu-trigger")).toBeNull();
  });

  it("opens a remote file tab from the remote tree", async () => {
    const opened: Array<[WorkspaceKey, string]> = [];
    await render(
      <FilesSurface
        workspaceKey={remoteKeyFor(DEVICE, WORKSPACE)}
        onOpenFile={(key, path) => opened.push([key, path])}
      />,
    );

    const row = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "note.txt",
    );
    if (!row) throw new Error("the remote file row did not render");
    await act(async () => {
      row.click();
    });

    expect(opened).toEqual([[remoteKeyFor(DEVICE, WORKSPACE), "note.txt"]]);
  });

  it("shows the host's status rows with no acts", async () => {
    await render(
      <ChangesSurface workspaceKey={remoteKeyFor(DEVICE, WORKSPACE)} canListCommits={false} />,
    );

    expect(remoteHostGitStatus).toHaveBeenCalledWith(DEVICE, WORKSPACE);
    expect(container.textContent).toContain("note.txt");
    expect(container.querySelector(".workspace-tree-menu-trigger")).toBeNull();
    expect(container.textContent).not.toContain("Stage");
  });

  it("edits a remote file end to end through the relay", async () => {
    await render(
      <WorkspaceFileTab
        workspaceKey={remoteKeyFor(DEVICE, WORKSPACE)}
        path="note.txt"
        refreshNonce={0}
        cache={new Map()}
      />,
    );

    expect(remoteHostFileOpen).toHaveBeenCalledWith(DEVICE, WORKSPACE, "note.txt");
    expect(container.querySelector('[data-testid="file-source-editor"]')).not.toBeNull();
  });

  it("keeps remote panels shut with a note against a stale daemon", async () => {
    vi.mocked(daemonStatus).mockResolvedValue({
      state: "connected",
      pid: 1,
      instanceId: "1",
      protocolVersion: 33,
      clients: 1,
      capabilities: [],
      message: null,
    } as never);

    await render(
      <FilesSurface workspaceKey={remoteKeyFor(DEVICE, WORKSPACE)} onOpenFile={() => undefined} />,
    );
    expect(remoteHostFilesList).not.toHaveBeenCalled();
    expect(container.textContent).toContain("Update the daemon to see this workspace.");
  });
});
