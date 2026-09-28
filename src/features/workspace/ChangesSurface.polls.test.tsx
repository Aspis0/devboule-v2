// @vitest-environment happy-dom

import { act, memo, type ComponentProps } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  WorkspaceGitCommitEntry,
  WorkspaceGitFileDiff,
  WorkspaceGitLog,
  WorkspaceGitRow,
  WorkspaceGitStatus,
} from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceGitStatus: vi.fn(),
  workspaceGitDiff: vi.fn(),
  workspaceGitStage: vi.fn(),
  workspaceGitUnstage: vi.fn(),
  workspaceGitDiscard: vi.fn(),
  workspaceGitCommit: vi.fn(),
  workspaceGitLog: vi.fn(),
}));

// The discard prop the surface hands the memoised tree, captured on every
// render through the real component: the forwarder re-renders with the
// surface, so its captures are exactly the identities the rows memoise on.
const capturedOnDiscard: Array<(paths: string[]) => void> = [];
vi.mock("./ChangesTreeView", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./ChangesTreeView")>();
  return {
    ...actual,
    ChangesTreeView: function CapturingChangesTreeView(
      props: ComponentProps<typeof actual.ChangesTreeView>,
    ) {
      capturedOnDiscard.push(props.onDiscard);
      return <actual.ChangesTreeView {...props} />;
    },
  };
});

import { ConfirmProvider } from "../../components/ConfirmHost";
import { useAppStore } from "../../store/appStore";
import {
  workspaceGitCommit,
  workspaceGitDiff,
  workspaceGitDiscard,
  workspaceGitLog,
  workspaceGitStage,
  workspaceGitStatus,
  workspaceGitUnstage,
} from "../../lib/tauri";
import { ChangesSurface } from "./ChangesSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "workspace-polls-subject";
const ROW_PATH = "notes/todo.md";

function commitEntry(overrides: Partial<WorkspaceGitCommitEntry> = {}): WorkspaceGitCommitEntry {
  return {
    sha: "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
    shortSha: "a1b2c3d",
    subject: "Add the thing",
    authorName: "gualt",
    authorDate: "2026-09-01T10:00:00+00:00",
    isOnRemote: true,
    isOnBase: false,
    ...overrides,
  };
}

function logReply(overrides: Partial<WorkspaceGitLog> = {}): WorkspaceGitLog {
  return {
    baseRef: "origin/main",
    commits: [commitEntry()],
    error: null,
    ...overrides,
  };
}

function statusReply(overrides: Partial<WorkspaceGitStatus> = {}): WorkspaceGitStatus {
  return {
    isGit: true,
    dirty: false,
    branch: "main",
    totals: { additions: 0, deletions: 0 },
    rows: [],
    error: null,
    ...overrides,
  };
}

function row(overrides: Partial<WorkspaceGitRow> & { path: string }): WorkspaceGitRow {
  return { additions: 0, deletions: 0, status: "untracked", capped: false, ...overrides };
}

function diffReply(overrides: Partial<WorkspaceGitFileDiff> = {}): WorkspaceGitFileDiff {
  return {
    path: ROW_PATH,
    isNew: false,
    isDeleted: false,
    additions: 0,
    deletions: 0,
    lines: [],
    status: "ok",
    error: null,
    ...overrides,
  };
}

// A memoised row's contract in one probe: it re-renders only when its
// props change identity, the same shallow check the tree's rows get.
let rowRenders = 0;
const RowProbe = memo(function RowProbe({ onDiscard }: { onDiscard: (paths: string[]) => void }) {
  rowRenders += 1;
  void onDiscard;
  return null;
});

describe("ChangesSurface no-op polls", () => {
  let container: HTMLDivElement;
  let root: Root;
  let probeContainer: HTMLDivElement;
  let probeRoot: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    probeContainer = document.createElement("div");
    document.body.appendChild(probeContainer);
    capturedOnDiscard.length = 0;
    rowRenders = 0;
    useAppStore.setState({ modalOpenTokens: new Set() });
    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply());
    vi.mocked(workspaceGitStage).mockResolvedValue(null);
    vi.mocked(workspaceGitUnstage).mockResolvedValue(null);
    vi.mocked(workspaceGitDiscard).mockResolvedValue(null);
    vi.mocked(workspaceGitCommit).mockResolvedValue(null);
    vi.mocked(workspaceGitLog).mockResolvedValue(logReply());
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    await act(async () => probeRoot.unmount());
    container.remove();
    probeContainer.remove();
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    useAppStore.setState({ modalOpenTokens: new Set() });
    vi.clearAllMocks();
  });

  async function render(ui: ReactNode) {
    root = createRoot(container);
    await act(async () => {
      root.render(<ConfirmProvider>{ui}</ConfirmProvider>);
    });
    probeRoot = createRoot(probeContainer);
  }

  async function refresh() {
    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('button[aria-label="Refresh"]')!
        .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => undefined);
  }

  // Three reads that change nothing — a fresh reply per read, the same row
  // objects inside — must not re-render a row: the discard prop the rows
  // memoise on holds still when the tree does.
  it("re-renders no row across three no-op polls", async () => {
    const stableRows = [row({ path: ROW_PATH, additions: 3 })];
    vi.mocked(workspaceGitStatus).mockImplementation(() =>
      Promise.resolve(
        statusReply({
          dirty: true,
          totals: { additions: 3, deletions: 0 },
          rows: [...stableRows],
        }),
      ),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);
    expect(container.querySelector(`.workspace-file-change[title="${ROW_PATH}"]`)).not.toBeNull();

    const latest = (): ((paths: string[]) => void) =>
      capturedOnDiscard[capturedOnDiscard.length - 1];
    await act(async () => {
      probeRoot.render(<RowProbe onDiscard={latest()} />);
    });
    expect(rowRenders).toBe(1);

    for (let poll = 0; poll < 3; poll++) {
      await refresh();
      await act(async () => {
        probeRoot.render(<RowProbe onDiscard={latest()} />);
      });
    }

    // The prop the rows memoise on never changed identity, across the
    // mount and all three reads.
    expect(capturedOnDiscard.length).toBeGreaterThan(1);
    for (const seen of capturedOnDiscard) expect(seen).toBe(capturedOnDiscard[0]);
    // So no extra row render happened: the probe counted every one.
    expect(rowRenders).toBe(1);
  });
});
