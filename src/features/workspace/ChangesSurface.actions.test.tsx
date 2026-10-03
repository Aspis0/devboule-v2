// @vitest-environment happy-dom

import { act } from "react";
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

// The confirmation belongs to the one act that loses data: the discard's
// gate lives inside `discard`, and these tests drive our dialog instead of
// a native mock — were the gate ever dropped from that road, the
// dialog-absence assertions below would die first. Stage, unstage and
// commit must never raise it at all, which their own assertions here prove.
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
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

/** The workspace as the UI names it, for a fixture that only knows the daemon id. */
const keyFor = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "workspace-git-actions-subject";
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

/** One dirty row on screen — the state every act below starts from. */
function dirtyReply(): WorkspaceGitStatus {
  return statusReply({
    dirty: true,
    totals: { additions: 3, deletions: 0 },
    rows: [row({ path: ROW_PATH, additions: 3 })],
  });
}

describe("ChangesSurface actions", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    useAppStore.setState({ modalOpenTokens: new Set() });
    vi.mocked(workspaceGitStatus).mockResolvedValue(dirtyReply());
    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply());
    vi.mocked(workspaceGitStage).mockResolvedValue(null);
    vi.mocked(workspaceGitUnstage).mockResolvedValue(null);
    vi.mocked(workspaceGitDiscard).mockResolvedValue(null);
    vi.mocked(workspaceGitCommit).mockResolvedValue(null);
    vi.mocked(workspaceGitLog).mockResolvedValue(logReply());
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    useAppStore.setState({ modalOpenTokens: new Set() });
    vi.clearAllMocks();
  });

  async function render(ui: ReactNode) {
    root = createRoot(container);
    await act(async () => {
      root.render(<ConfirmProvider>{ui}</ConfirmProvider>);
    });
  }

  function button(selector: string): HTMLButtonElement {
    const found = container.querySelector<HTMLButtonElement>(selector);
    if (found === null) throw new Error(`no button: ${selector}`);
    return found;
  }

  /** The toolbar's Commit button, found by its own label (the only one). */
  function commitButton(): HTMLButtonElement {
    const found = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === "Commit",
    );
    if (found === undefined) throw new Error("no commit button");
    return found;
  }

  /** Open a row's menu — the only way to Discard exists behind it. */
  async function openMenu(path: string = ROW_PATH) {
    await act(async () => {
      button(`button[aria-label="${path} actions"]`).click();
    });
    expect(container.querySelector('[role="menu"]')).not.toBeNull();
  }

  /** The standing ask, portaled to the body — never inside the panel. */
  function confirmDialog(): HTMLElement {
    const found = document.querySelector<HTMLElement>(".confirm-dialog");
    if (found === null) throw new Error("confirm dialog did not render");
    return found;
  }

  function affirmative(): HTMLButtonElement {
    const found = document.querySelector<HTMLButtonElement>(".confirm-dialog-confirm");
    if (found === null) throw new Error("confirm button did not render");
    return found;
  }

  function cancel(): HTMLButtonElement {
    const found = document.querySelector<HTMLButtonElement>(".confirm-dialog-cancel");
    if (found === null) throw new Error("cancel button did not render");
    return found;
  }

  /** Choose Discard behind the open menu — the ask stands unanswered. */
  async function chooseDiscard(): Promise<void> {
    await act(async () => {
      container.querySelector<HTMLElement>('[role="menuitem"]')?.click();
    });
  }

  async function answerConfirm(): Promise<void> {
    await act(async () => {
      affirmative().click();
    });
  }

  async function answerCancel(): Promise<void> {
    await act(async () => {
      cancel().click();
    });
  }

  async function escapeAsk(): Promise<void> {
    await act(async () => {
      confirmDialog().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
  }

  async function scrimAsk(): Promise<void> {
    await act(async () => {
      document
        .querySelector<HTMLElement>(".confirm-dialog-backdrop")!
        .dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });
  }

  // The gate is structural: choosing Discard raises our dialog, and while
  // the ask stands unanswered no command has reached the wire — the discard
  // spy stays quiet beside the open dialog. Mutation that kills it: drop
  // the ask from `discard` (call the command directly) — the wire assertion
  // below fails on the open ask.
  it("asks through our dialog and calls no wire before the answer", async () => {
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);
    // The tree shows the basename; the full path rides the row's title.
    expect(container.querySelector(`.workspace-file-change[title="${ROW_PATH}"]`)).not.toBeNull();

    await openMenu();
    await chooseDiscard();

    const card = confirmDialog();
    expect(card.querySelector(".confirm-dialog-title")?.textContent).toBe("Discard changes");
    expect(card.querySelector(".confirm-dialog-body")?.textContent).toContain(ROW_PATH);
    expect(card.querySelector(".confirm-dialog-body")?.textContent).toContain(
      "This cannot be undone.",
    );
    expect(affirmative().textContent).toBe("Discard");
    expect(cancel().textContent).toBe("Keep them");
    expect(affirmative().classList.contains("confirm-dialog-confirm-danger")).toBe(true);
    expect(vi.mocked(workspaceGitDiscard)).not.toHaveBeenCalled();
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(1);
  });

  // Every way out but the affirmative declines: no wire, not even a
  // refresh (a No means nothing happened, so even the status re-read would
  // be a lie), the row intact, the ask gone.
  it("Cancel, Escape and the scrim decline: no wire, no refresh, the row intact", async () => {
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);

    await openMenu();
    await chooseDiscard();
    await answerCancel();
    expect(vi.mocked(workspaceGitDiscard)).not.toHaveBeenCalled();
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    expect(container.querySelector(`.workspace-file-change[title="${ROW_PATH}"]`)).not.toBeNull();

    await openMenu();
    await chooseDiscard();
    await escapeAsk();
    expect(vi.mocked(workspaceGitDiscard)).not.toHaveBeenCalled();
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".confirm-dialog")).toBeNull();

    await openMenu();
    await chooseDiscard();
    await scrimAsk();
    expect(vi.mocked(workspaceGitDiscard)).not.toHaveBeenCalled();
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    expect(container.querySelector(`.workspace-file-change[title="${ROW_PATH}"]`)).not.toBeNull();
  });

  // The ask borrows focus and hands it back: Cancel lands on the row's
  // menu trigger the menu opened from — the dialog stays mounted and
  // closes through its `open` prop, so its own trigger return runs.
  it("Cancel hands focus back to the row's menu trigger", async () => {
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);
    const trigger = button(`button[aria-label="${ROW_PATH} actions"]`);
    await act(async () => {
      trigger.focus();
    });

    await openMenu();
    await chooseDiscard();
    expect(document.activeElement).toBe(cancel());

    await answerCancel();
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  // A mouse press moves focus onto the menu item before the click reaches
  // it; the item dies with the menu, so without the trigger's pre-focus
  // the dialog would capture a gone element and strand focus on <body>.
  it("returns focus to the trigger even when the menu item held focus at the ask", async () => {
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);
    const trigger = button(`button[aria-label="${ROW_PATH} actions"]`);
    await act(async () => {
      trigger.focus();
    });

    await openMenu();
    const item = container.querySelector<HTMLElement>('[role="menuitem"]');
    if (item === null) throw new Error("discard menu item did not render");
    await act(async () => {
      item.focus();
    });
    await chooseDiscard();
    expect(document.activeElement).toBe(cancel());

    await answerCancel();
    expect(document.activeElement).toBe(trigger);
  });

  // A landed discard can take its own row with it: the refresh re-reads
  // clean, the trigger is gone, and focus parks on the panel — never body.
  it("a confirmed discard that empties the tree parks focus on the panel", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValueOnce(dirtyReply());
    vi.mocked(workspaceGitStatus).mockResolvedValue(statusReply());
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);
    const trigger = button(`button[aria-label="${ROW_PATH} actions"]`);
    await act(async () => {
      trigger.focus();
    });

    await openMenu();
    await chooseDiscard();
    await answerConfirm();
    await act(async () => undefined);

    expect(vi.mocked(workspaceGitDiscard)).toHaveBeenCalledTimes(1);
    expect(container.querySelector(`.workspace-file-change[title="${ROW_PATH}"]`)).toBeNull();
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    const panel = container.querySelector<HTMLElement>(".workspace-changes");
    expect(panel).not.toBeNull();
    expect(document.activeElement).toBe(panel);
    expect(document.activeElement).not.toBe(document.body);
  });

  // A slow act parks focus on the panel for the whole wire latency —
  // never on the body — and hands it back to the surviving trigger once
  // the refresh lands. The status mock answers fresh objects per read, the
  // way the daemon's replies always do, so the refresh changes identity.
  it("a slow act parks on the panel and returns focus to the row", async () => {
    vi.mocked(workspaceGitStatus).mockImplementation(() => Promise.resolve(dirtyReply()));
    let resolveWire: ((value: string | null) => void) | null = null;
    vi.mocked(workspaceGitDiscard).mockImplementation(
      () =>
        new Promise<string | null>((resolve) => {
          resolveWire = resolve;
        }),
    );
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);
    const trigger = button(`button[aria-label="${ROW_PATH} actions"]`);
    await act(async () => {
      trigger.focus();
    });

    await openMenu();
    await chooseDiscard();
    await answerConfirm();
    expect(vi.mocked(workspaceGitDiscard)).toHaveBeenCalledTimes(1);
    const panel = container.querySelector<HTMLElement>(".workspace-changes");
    expect(panel).not.toBeNull();
    expect(document.activeElement).toBe(panel);

    await act(async () => {
      resolveWire!(null);
    });
    await act(async () => undefined);
    expect(container.querySelector(`.workspace-file-change[title="${ROW_PATH}"]`)).not.toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  // The real order: the wire answers on its own latency and the re-read
  // lands after it — two separate commits, not one. The arm must survive
  // the flip on the stale rows and settle the row-gone case onto the panel
  // once the re-read arrives. A harness that answers both at once batches
  // them into one commit and never exercises this.
  it("a slow confirmed discard that takes its row lands on the panel once the re-read lands", async () => {
    let wireDone = false;
    let releaseRefresh: ((value: WorkspaceGitStatus) => void) | null = null;
    vi.mocked(workspaceGitStatus).mockImplementation(
      () =>
        new Promise<WorkspaceGitStatus>((resolve) => {
          if (!wireDone) resolve(dirtyReply());
          else releaseRefresh = resolve;
        }),
    );
    let resolveWire: ((value: string | null) => void) | null = null;
    vi.mocked(workspaceGitDiscard).mockImplementation(
      () =>
        new Promise<string | null>((resolve) => {
          resolveWire = resolve;
        }),
    );
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);
    const trigger = button(`button[aria-label="${ROW_PATH} actions"]`);
    await act(async () => {
      trigger.focus();
    });

    await openMenu();
    await chooseDiscard();
    await answerConfirm();
    expect(vi.mocked(workspaceGitDiscard)).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    // The dialog unregistered before the wire started: the park reads a
    // count without the ask in it.
    expect(useAppStore.getState().modalOpenTokens.size).toBe(0);
    const panel = container.querySelector<HTMLElement>(".workspace-changes");
    expect(panel).not.toBeNull();
    expect(document.activeElement).toBe(panel);

    await act(async () => {
      wireDone = true;
      resolveWire!(null);
    });
    await act(async () => undefined);
    // The flip lands on the stale tree first: the repair borrows the row's
    // own trigger back until the re-read arrives.
    expect(document.activeElement).toBe(trigger);

    await act(async () => {
      releaseRefresh!(statusReply());
    });
    await act(async () => undefined);
    expect(container.querySelector(`.workspace-file-change[title="${ROW_PATH}"]`)).toBeNull();
    expect(document.activeElement).toBe(panel);
    expect(document.activeElement).not.toBe(document.body);
  });

  // The live shape behind this pass: the act's own re-read still lists
  // the row — the arm settles on a survivor — and a later refresh takes
  // it. The repair had put focus back on the trigger, so the removal
  // strands it; the panel root reclaims it with no arm behind it.
  it("a row that survives the act but leaves on a later refresh lands on the panel", async () => {
    let phase: "before" | "reread" | "after" = "before";
    let releaseReread: ((value: WorkspaceGitStatus) => void) | null = null;
    vi.mocked(workspaceGitStatus).mockImplementation(() => {
      if (phase === "before") return Promise.resolve(dirtyReply());
      if (phase === "reread")
        return new Promise<WorkspaceGitStatus>((resolve) => {
          releaseReread = resolve;
        });
      return Promise.resolve(statusReply());
    });
    let resolveWire: ((value: string | null) => void) | null = null;
    vi.mocked(workspaceGitDiscard).mockImplementation(
      () =>
        new Promise<string | null>((resolve) => {
          resolveWire = resolve;
        }),
    );
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);
    const trigger = button(`button[aria-label="${ROW_PATH} actions"]`);
    await act(async () => {
      trigger.focus();
    });

    await openMenu();
    await chooseDiscard();
    phase = "reread";
    await answerConfirm();
    const panel = container.querySelector<HTMLElement>(".workspace-changes");
    expect(panel).not.toBeNull();
    expect(document.activeElement).toBe(panel);

    await act(async () => {
      resolveWire!(null);
    });
    await act(async () => undefined);
    // The flip lands on the stale tree first: the repair borrows the
    // row's own trigger back until the re-read arrives.
    expect(document.activeElement).toBe(trigger);

    await act(async () => {
      releaseReread!(dirtyReply());
    });
    await act(async () => undefined);
    // The row survives the act: still listed, still focused, the arm
    // spent on a survivor.
    expect(container.querySelector(`.workspace-file-change[title="${ROW_PATH}"]`)).not.toBeNull();
    expect(document.activeElement).toBe(trigger);

    // A later refresh takes the row: no arm is left, and the panel root
    // reclaims the stranded focus anyway.
    phase = "after";
    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('button[aria-label="Refresh"]')!
        .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => undefined);
    expect(container.querySelector(`.workspace-file-change[title="${ROW_PATH}"]`)).toBeNull();
    expect(document.activeElement).toBe(panel);
    expect(document.activeElement).not.toBe(document.body);
  });

  // A modal holding the keyboard vetoes the rescue: the row leaves under
  // it and focus stays where the removal dropped it.
  it("moves no focus when the row leaves while a modal is open", async () => {
    vi.mocked(workspaceGitStatus).mockImplementation(() => Promise.resolve(dirtyReply()));
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);
    const trigger = button(`button[aria-label="${ROW_PATH} actions"]`);
    await act(async () => {
      trigger.focus();
    });
    await act(async () => {
      useAppStore.setState({ modalOpenTokens: new Set(["probe-modal"]) });
    });

    vi.mocked(workspaceGitStatus).mockImplementation(() => Promise.resolve(statusReply()));
    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('button[aria-label="Refresh"]')!
        .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => undefined);
    expect(container.querySelector(`.workspace-file-change[title="${ROW_PATH}"]`)).toBeNull();
    expect(document.activeElement).toBe(document.body);
    expect(document.activeElement).not.toBe(container.querySelector(".workspace-changes"));
  });

  // Some engines report a focused removal as a focusout onto the body
  // instead of no event at all: the stash must survive that report, so
  // the rescue still reclaims the panel when the next tree lands.
  it("reclaims the panel when a removal reports its focusout onto the body", async () => {
    vi.mocked(workspaceGitStatus).mockImplementation(() => Promise.resolve(dirtyReply()));
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);
    const trigger = button(`button[aria-label="${ROW_PATH} actions"]`);
    await act(async () => {
      trigger.focus();
    });
    await act(async () => {
      trigger.dispatchEvent(
        new FocusEvent("focusout", { bubbles: true, relatedTarget: document.body }),
      );
    });

    vi.mocked(workspaceGitStatus).mockImplementation(() => Promise.resolve(statusReply()));
    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('button[aria-label="Refresh"]')!
        .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => undefined);
    expect(container.querySelector(`.workspace-file-change[title="${ROW_PATH}"]`)).toBeNull();
    expect(document.activeElement).toBe(container.querySelector(".workspace-changes"));
    expect(document.activeElement).not.toBe(document.body);
  });

  // A focus the person moved mid-act is theirs: resolving the wire must
  // not yank it back to the row.
  it("leaves a focus the person moved mid-act where they put it", async () => {
    vi.mocked(workspaceGitStatus).mockImplementation(() => Promise.resolve(dirtyReply()));
    let resolveWire: ((value: string | null) => void) | null = null;
    vi.mocked(workspaceGitDiscard).mockImplementation(
      () =>
        new Promise<string | null>((resolve) => {
          resolveWire = resolve;
        }),
    );
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);
    const trigger = button(`button[aria-label="${ROW_PATH} actions"]`);
    await act(async () => {
      trigger.focus();
    });

    await openMenu();
    await chooseDiscard();
    await answerConfirm();
    expect(document.activeElement).toBe(container.querySelector(".workspace-changes"));

    const elsewhere = document.createElement("button");
    elsewhere.textContent = "elsewhere";
    document.body.appendChild(elsewhere);
    await act(async () => {
      elsewhere.focus();
    });
    await act(async () => {
      resolveWire!(null);
    });
    await act(async () => undefined);
    expect(document.activeElement).toBe(elsewhere);
    elsewhere.remove();
  });

  // A rows change with no ask behind it — a menu opened and dismissed, a
  // poll re-read landing different rows — moves no focus. The Refresh
  // click is dispatched, not clicked, so the harness moves no focus of
  // its own: the only focus change this test could see is the hook's.
  it("moves no focus for a rows change with no ask behind it", async () => {
    vi.mocked(workspaceGitStatus).mockImplementation(() => Promise.resolve(dirtyReply()));
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);

    await openMenu();
    await act(async () => {
      button(`button[aria-label="${ROW_PATH} actions"]`).click();
    });
    expect(container.querySelector('[role="menu"]')).toBeNull();
    (document.activeElement as HTMLElement | null)?.blur();
    expect(document.activeElement).toBe(document.body);

    vi.mocked(workspaceGitStatus).mockImplementation(() =>
      Promise.resolve(
        statusReply({
          dirty: true,
          totals: { additions: 1, deletions: 0 },
          rows: [row({ path: "other/file.md", additions: 1 })],
        }),
      ),
    );
    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('button[aria-label="Refresh"]')!
        .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => undefined);
    expect(container.querySelector(`.workspace-file-change[title="other/file.md"]`)).not.toBeNull();
    expect(document.activeElement).toBe(document.body);
  });

  // The discard question names the full workspace-relative path, two
  // levels deep: a body built from the basename would fail this.
  it("names the full nested path in the discard question", async () => {
    const nested = "src/deep/file.ts";
    vi.mocked(workspaceGitStatus).mockImplementation(() =>
      Promise.resolve(
        statusReply({
          dirty: true,
          totals: { additions: 1, deletions: 0 },
          rows: [row({ path: nested, additions: 1 })],
        }),
      ),
    );
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);

    await openMenu(nested);
    await chooseDiscard();
    expect(confirmDialog().querySelector(".confirm-dialog-body")?.textContent).toContain(nested);
    expect(vi.mocked(workspaceGitDiscard)).not.toHaveBeenCalled();
    await answerConfirm();
    expect(vi.mocked(workspaceGitDiscard)).toHaveBeenCalledWith(WORKSPACE, [nested]);
  });

  // The affirmative acts exactly once, with this row's path, and the
  // immediate refresh the panel owes follows without any timer being
  // advanced (the poll is 5 s; this test runs in milliseconds).
  it("the affirmative discards once and re-reads the status immediately", async () => {
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);

    await openMenu();
    await chooseDiscard();
    expect(vi.mocked(workspaceGitDiscard)).not.toHaveBeenCalled();
    await answerConfirm();

    expect(vi.mocked(workspaceGitDiscard)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceGitDiscard)).toHaveBeenCalledWith(WORKSPACE, [ROW_PATH]);
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(2);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
  });

  // Unmounting with the ask standing (a workspace or panel switch remounts
  // the host) declines it: no wire, and the modal token goes with the dialog.
  it("unmounting with the ask standing declines it and leaks no modal", async () => {
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);

    await openMenu();
    await chooseDiscard();
    expect(document.querySelector(".confirm-dialog")).not.toBeNull();
    await act(async () => {
      root.unmount();
    });

    expect(vi.mocked(workspaceGitDiscard)).not.toHaveBeenCalled();
    expect(useAppStore.getState().modalOpenTokens.size).toBe(0);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
  });

  // The brief's second UI case: after Stage the panel re-reads at once and
  // the row's own answer changes with it — first reply has the row, second
  // (the refresh's) does not. Mutation that kills it: drop the `refresh()`
  // from the writer hook — the second call never happens and the row never
  // leaves. Stage itself asks no confirmation: only discard does.
  it("stages without asking and re-reads the status immediately, not on the poll", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValueOnce(dirtyReply());
    vi.mocked(workspaceGitStatus).mockResolvedValueOnce(statusReply());
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);
    expect(container.querySelector(`.workspace-file-change[title="${ROW_PATH}"]`)).not.toBeNull();

    await act(async () => {
      button(`button[title="Stage ${ROW_PATH}"]`).click();
    });

    expect(document.querySelector(".confirm-dialog")).toBeNull();
    expect(vi.mocked(workspaceGitStage)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceGitStage)).toHaveBeenCalledWith(WORKSPACE, [ROW_PATH]);
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(2);
    expect(container.textContent).toContain("No uncommitted changes in this workspace.");
    expect(container.querySelector(`.workspace-file-change[title="${ROW_PATH}"]`)).toBeNull();
  });

  // The review's §4.1 defect, fixed end to end from the row down to the
  // wire: a renamed row acts on **both** of its paths, because the new one
  // alone leaves the old side's deletion staged and the only copy on disk
  // deleted — a half operation that answered success (measured on git
  // 2.54.0). The old path is the status reply's own `renamedFrom` (the
  // bare `-z` token the parse used to drop), and the confirmation names
  // both files it will touch. Mutant `e:1` — drop the partner from
  // `pathsOf`: the wire spy below receives a one-element array and the
  // first equality fails.
  it("sends both paths of a renamed row to the wire, stage and discard alike", async () => {
    const renamed: WorkspaceGitRow = {
      path: "notes/todo-v2.md",
      renamedFrom: "notes/todo-v1.md",
      additions: 3,
      deletions: 0,
      status: "renamed",
      capped: false,
    };
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 3, deletions: 0 },
        rows: [renamed],
      }),
    );
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);
    expect(
      container.querySelector('.workspace-file-change[title="notes/todo-v2.md"]'),
    ).not.toBeNull();

    await act(async () => {
      button('button[title="Stage notes/todo-v2.md"]').click();
    });
    expect(vi.mocked(workspaceGitStage)).toHaveBeenCalledWith(WORKSPACE, [
      "notes/todo-v2.md",
      "notes/todo-v1.md",
    ]);

    await openMenu("notes/todo-v2.md");
    await chooseDiscard();
    // The question itself names the file the user never clicked: both
    // sides are about to disappear — and nothing has reached the wire yet.
    expect(confirmDialog().querySelector(".confirm-dialog-body")?.textContent).toContain(
      "notes/todo-v1.md",
    );
    expect(confirmDialog().querySelector(".confirm-dialog-body")?.textContent).toContain(
      "notes/todo-v2.md",
    );
    expect(vi.mocked(workspaceGitDiscard)).not.toHaveBeenCalled();
    await answerConfirm();

    expect(vi.mocked(workspaceGitDiscard)).toHaveBeenCalledWith(WORKSPACE, [
      "notes/todo-v2.md",
      "notes/todo-v1.md",
    ]);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
  });

  // Enter during an IME composition is the candidate list's key: it must
  // not reach the commit, and the typed message stays untouched.
  it("leaves Enter to an open IME composition instead of committing", async () => {
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);

    const input = container.querySelector<HTMLInputElement>('[aria-label="Commit message"]');
    if (input === null) throw new Error("no message field");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set;
    if (setter === undefined) throw new Error("no value setter");
    await act(async () => {
      setter.call(input, "say what changed");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });

    await act(async () => {
      input.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          bubbles: true,
          cancelable: true,
          isComposing: true,
        }),
      );
    });
    expect(vi.mocked(workspaceGitCommit)).not.toHaveBeenCalled();
    expect(input.value).toBe("say what changed");

    // Older engines report the composition commit as keyCode 229 alone.
    await act(async () => {
      input.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          bubbles: true,
          cancelable: true,
          keyCode: 229,
        }),
      );
    });
    expect(vi.mocked(workspaceGitCommit)).not.toHaveBeenCalled();
    expect(input.value).toBe("say what changed");

    // Composition closed: the next Enter commits, as it always has.
    await act(async () => {
      input.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
      );
    });
    expect(vi.mocked(workspaceGitCommit)).toHaveBeenCalledWith(WORKSPACE, "say what changed");
  });

  // Commit's own discipline at the keyboard: an empty message never
  // reaches the wire (the button is disabled for it — the daemon refuses
  // it again, but the panel should not offer the trip), a written one is
  // sent verbatim, and a landed commit clears the field so the next one
  // starts honest.
  it("refuses an empty commit message at the toolbar and sends a written one verbatim", async () => {
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);

    const commit = commitButton();
    expect(commit.disabled).toBe(true);
    await act(async () => {
      commit.click();
    });
    expect(vi.mocked(workspaceGitCommit)).not.toHaveBeenCalled();

    const input = container.querySelector<HTMLInputElement>('[aria-label="Commit message"]');
    if (input === null) throw new Error("no message field");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set;
    if (setter === undefined) throw new Error("no value setter");
    await act(async () => {
      setter.call(input, "say what changed");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });

    expect(commitButton().disabled).toBe(false);
    await act(async () => {
      commitButton().click();
    });

    expect(vi.mocked(workspaceGitCommit)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceGitCommit)).toHaveBeenCalledWith(WORKSPACE, "say what changed");
    // Commit asks for nothing: only discard raises the dialog.
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    expect(container.querySelector<HTMLInputElement>('[aria-label="Commit message"]')?.value).toBe(
      "",
    );
  });

  // R7d: a commit is history the moment it lands — the Commits view's
  // list is stale whether or not that view is the one on screen, so the
  // read is repeated beside the status refresh the act already owes.
  it("refetches the history after a commit lands", async () => {
    await render(<ChangesSurface workspaceKey={keyFor(WORKSPACE)} canListCommits={true} />);
    expect(vi.mocked(workspaceGitLog)).not.toHaveBeenCalled();

    const input = container.querySelector<HTMLInputElement>('[aria-label="Commit message"]');
    if (input === null) throw new Error("no message field");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set;
    if (setter === undefined) throw new Error("no value setter");
    await act(async () => {
      setter.call(input, "ship it");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => {
      commitButton().click();
    });

    expect(vi.mocked(workspaceGitCommit)).toHaveBeenCalledWith(WORKSPACE, "ship it");
    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledWith(WORKSPACE);
  });
});
