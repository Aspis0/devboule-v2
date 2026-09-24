// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { JournalUsage, ResumeResult, Session } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  isCommandError: vi.fn(
    (error: unknown) =>
      typeof error === "object" && error !== null && "code" in error && "message" in error,
  ),
  journalUsage: vi.fn(),
  sessionDelete: vi.fn(),
  sessionResume: vi.fn(),
  sessionsList: vi.fn(),
}));

import { journalUsage, sessionDelete, sessionResume, sessionsList } from "../../lib/tauri";
import { HistoryPanel } from "./HistoryPanel";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const now = new Date(2026, 8, 4, 12, 0, 0, 0).getTime();

let container: HTMLDivElement;
let root: Root;

function endedSession(id: string): Session {
  return {
    id,
    workspaceId: "workspace-rust",
    kind: "terminal",
    title: "joined title",
    state: {
      type: "ended",
      generation: 1,
      code: 0,
      integrity: { kind: "complete" },
    },
    elapsedMs: 0,
  };
}

function resumableSession(id: string): Session {
  return {
    ...endedSession(id),
    kind: "acp",
    provider: "grok",
    peerSessionId: "peer-session-1",
    resumable: true,
  };
}

function baseUsage(): JournalUsage {
  return {
    totalBytes: 12_345,
    sessionCount: 2,
    deletedByUser: 0,
    deletedByRetention: 0,
    unreclaimable: { bytesOver: 0, sessionsOver: 0, agedOut: 0 },
    limits: {
      snapshotEveryBytes: 65_536,
      sessionMaxBytes: 512 * 1024 * 1024,
      maxBytes: 8 * 1024 * 1024 * 1024,
      maxSessions: 10_000,
      maxAgeMs: 0,
    },
    perSession: [
      {
        id: "session-build",
        title: "Build history",
        kind: "terminal",
        bytes: 400,
        updatedAtMs: now,
      },
      {
        id: "session-review",
        title: "Review history",
        kind: "acp",
        bytes: 500,
        updatedAtMs: new Date(2026, 8, 3, 12).getTime(),
      },
    ],
  };
}

function renderPanel(usage: JournalUsage = baseUsage(), sessions: Session[] = []) {
  vi.mocked(journalUsage).mockResolvedValueOnce(usage);
  vi.mocked(sessionsList).mockResolvedValueOnce(sessions);
  root = createRoot(container);
  return act(async () => {
    root.render(<HistoryPanel now={now} search="" />);
    await Promise.resolve();
  });
}

function buttonByLabel(label: string): HTMLButtonElement {
  const button = container.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
  if (!button) throw new Error(`button ${label} did not render`);
  return button;
}

describe("HistoryPanel", () => {
  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(sessionDelete).mockResolvedValue(undefined);
    vi.mocked(sessionResume).mockResolvedValue({
      type: "resumed",
      session: resumableSession("session-review"),
    });
  });

  afterEach(async () => {
    vi.useRealTimers();
    await act(async () => root?.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it("renders session titles and groups them under day headings", async () => {
    await renderPanel();
    expect(container.textContent).toContain("Build history");
    expect(container.textContent).toContain("Review history");
    expect(container.textContent).toContain("Today");
    expect(container.textContent).toContain("Yesterday");
  });

  it("filters rows from the controlled search prop", async () => {
    await renderPanel();
    await act(async () => root.render(<HistoryPanel now={now} search="review" />));
    expect(container.textContent).toContain("Review history");
    expect(container.textContent).not.toContain("Build history");
    expect(container.textContent).not.toContain("Yesterday");
  });

  it("shows the display name a history row carries, not only its title", async () => {
    const usage = baseUsage();
    usage.perSession = [
      { ...usage.perSession[0], id: "session-child", title: "child", displayName: "worker one" },
    ];
    await renderPanel(usage);

    // The name the tab strip already shows; the title is not painted beside it.
    expect(container.textContent).toContain("worker one");
    expect(container.textContent).not.toContain("child");
  });

  it("filters on the name the row shows, and still on the title it hides", async () => {
    const usage = baseUsage();
    usage.perSession = [
      { ...usage.perSession[0], id: "session-child", title: "child", displayName: "worker one" },
    ];
    await renderPanel(usage);

    // What the user just read finds the row...
    await act(async () => root.render(<HistoryPanel now={now} search="worker one" />));
    expect(container.textContent).toContain("worker one");
    // ...and the title it is shown under instead of still finds it.
    await act(async () => root.render(<HistoryPanel now={now} search="child" />));
    expect(container.textContent).toContain("worker one");
    // A query that matches neither name nor metadata filters it out.
    await act(async () => root.render(<HistoryPanel now={now} search="nobody" />));
    expect(container.textContent).not.toContain("worker one");
  });

  it("falls back to the title, then to the kind-derived name, when no display name is set", async () => {
    const usage = baseUsage();
    usage.perSession = [
      { ...usage.perSession[0], id: "session-plain", title: "Build history" },
      { ...usage.perSession[1], id: "session-blank", title: "Review history", displayName: "   " },
      {
        ...usage.perSession[1],
        id: "s.4242.7",
        title: "   ",
        kind: "acp",
        displayName: " ",
      },
    ];
    await renderPanel(usage);

    // No display name at all: the title, exactly as before the field existed.
    expect(container.textContent).toContain("Build history");
    // A display name that is only whitespace is no name: the title again.
    expect(container.textContent).toContain("Review history");
    // Neither name: the kind-derived fallback, never an empty label.
    expect(container.textContent).toContain("Agent s.4242.7");
  });

  it("shows total saved bytes and the saved session count", async () => {
    await renderPanel();
    expect(container.textContent).toContain("12 345");
    expect(container.textContent).toContain("2");
  });

  it("shows Reopen only where the daemon's verdict says resume", async () => {
    const usage = baseUsage();
    usage.perSession = [
      usage.perSession[0],
      usage.perSession[1],
      { ...usage.perSession[1], id: "session-claude", title: "Claude row" },
      { ...usage.perSession[1], id: "session-live", title: "Live row" },
      { ...usage.perSession[1], id: "session-old", title: "Old daemon row" },
    ];
    await renderPanel(usage, [
      endedSession("session-build"),
      resumableSession("session-review"),
      // Claude resumes on the same verdict: the kind is not the decision.
      { ...resumableSession("session-claude"), kind: "claude", provider: "claude" },
      // Live, with columns: the daemon refuses while the process runs.
      {
        ...resumableSession("session-live"),
        state: { type: "live", generation: 1 },
        resumable: false,
      },
      // A daemon that predates the field says nothing: no button on a guess.
      { ...resumableSession("session-old"), resumable: undefined },
    ]);
    expect(container.querySelectorAll(".history-reopen-action")).toHaveLength(2);
    expect(container.textContent).toContain("Reopen");
  });

  it("resumes the selected history row exactly once and hands the session to onReopen", async () => {
    const onReopen = vi.fn();
    const usage = baseUsage();
    usage.perSession = [usage.perSession[1]];
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([resumableSession("session-review")]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" onReopen={onReopen} />);
      await Promise.resolve();
    });
    const reopen = container.querySelector<HTMLButtonElement>(".history-reopen-action");
    if (!reopen) throw new Error("reopen control did not render");
    await act(async () => reopen.click());
    expect(sessionResume).toHaveBeenCalledTimes(1);
    expect(sessionResume).toHaveBeenCalledWith("session-review");
    expect(onReopen).toHaveBeenCalledTimes(1);
    expect(onReopen).toHaveBeenCalledWith(
      expect.objectContaining({ id: "session-review", kind: "acp" }),
    );
  });

  it("shows the mapped sentence when resuming fails, never the raw daemon text", async () => {
    const cause = { code: "invalid_request", message: "provider session is unavailable" };
    vi.mocked(sessionResume).mockRejectedValueOnce(cause);
    const usage = baseUsage();
    usage.perSession = [usage.perSession[1]];
    await renderPanel(usage, [resumableSession("session-review")]);
    const reopen = container.querySelector<HTMLButtonElement>(".history-reopen-action");
    if (!reopen) throw new Error("reopen control did not render");
    await act(async () => reopen.click());
    await act(async () => undefined);
    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("The agent daemon refused that request as invalid.");
    // The raw daemon text is kept, demoted into the visually hidden detail.
    expect(alert?.querySelector(".error-detail-sr-only")?.textContent).toBe(
      "provider session is unavailable",
    );
  });

  it("re-reads the roster when a resume fails, so the offer cannot outlive the verdict", async () => {
    const cause = { code: "io", message: "ACP request failed (-32002): Resource not found" };
    vi.mocked(sessionResume).mockRejectedValueOnce(cause);
    const usage = baseUsage();
    usage.perSession = [usage.perSession[1]];
    await renderPanel(usage, [resumableSession("session-review")]);
    expect(sessionsList).toHaveBeenCalledTimes(1);

    // The refreshed roster carries the daemon's retraction.
    vi.mocked(sessionsList).mockResolvedValueOnce([
      { ...resumableSession("session-review"), resumable: false },
    ]);
    const reopen = container.querySelector<HTMLButtonElement>(".history-reopen-action");
    if (!reopen) throw new Error("reopen control did not render");
    await act(async () => reopen.click());
    await act(async () => undefined);

    try {
      expect(sessionsList).toHaveBeenCalledTimes(2);
      expect(container.querySelector(".history-reopen-action")).toBeNull();
    } finally {
      // This test's refresh value is queued behind a call only the production
      // makes; reset instead of leaving the queue to the next test.
      vi.mocked(sessionsList).mockReset();
    }
  });

  it("ignores a second Reopen click while resume is in flight", async () => {
    let resolveResume: ((result: ResumeResult) => void) | undefined;
    vi.mocked(sessionResume).mockReturnValue(
      new Promise<ResumeResult>((resolve) => {
        resolveResume = resolve;
      }),
    );
    const usage = baseUsage();
    usage.perSession = [usage.perSession[1]];
    await renderPanel(usage, [resumableSession("session-review")]);
    const reopen = container.querySelector<HTMLButtonElement>(".history-reopen-action");
    if (!reopen) throw new Error("reopen control did not render");
    await act(async () => reopen.click());
    await act(async () => reopen.click());
    expect(sessionResume).toHaveBeenCalledTimes(1);
    await act(async () => resolveResume?.({ type: "not_supported" }));
  });

  it("declares deleted sessions and unreclaimable sessions", async () => {
    const usage = baseUsage();
    usage.deletedByRetention = 3;
    usage.unreclaimable.sessionsOver = 2;
    await renderPanel(usage);
    expect(container.textContent).toContain("The history limit removed 3 sessions.");
    expect(container.textContent).toContain("Retention cannot reclaim 2 sessions");
  });

  it("renders the history-limit notice when retention removed sessions", async () => {
    const usage = baseUsage();
    usage.deletedByRetention = 3;
    usage.deletedByUser = 0;
    await renderPanel(usage);
    expect(container.textContent).toContain("The history limit removed 3 sessions.");
    expect(container.textContent).not.toContain("sessions were removed from history");
  });

  it("does not render a history notice for user-only deletions", async () => {
    const usage = baseUsage();
    usage.deletedByUser = 4;
    usage.deletedByRetention = 0;
    await renderPanel(usage);
    expect(container.querySelector(".history-notice")).toBeNull();
    expect(container.textContent).not.toContain("sessions were removed from history");
    expect(container.textContent).not.toContain("The history limit removed");
  });

  it("disables delete for a joined live session with an archive-first explanation", async () => {
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    await renderPanel(usage, [
      {
        ...endedSession("session-build"),
        state: { type: "live", generation: 1 },
      },
    ]);
    const label = "Archive the session before deleting it from history.";
    const button = buttonByLabel(label);
    expect(button.disabled).toBe(true);
    expect(button.title).toBe(label);
  });

  it("requires same-row confirmation before deleting an ended session", async () => {
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    await renderPanel(usage, [endedSession("session-build")]);
    const initial = container.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!initial) throw new Error("delete control did not render");
    await act(async () => initial.click());
    expect(sessionDelete).not.toHaveBeenCalled();
    const confirm = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".history-delete-action"),
    ).find((button) => button.textContent === "Delete from history");
    if (!confirm) throw new Error("delete confirmation did not render");
    await act(async () => confirm.click());
    expect(sessionDelete).toHaveBeenCalledWith("session-build");
  });

  it("shows a reason when deleting fails", async () => {
    const cause = { code: "invalid_request", message: "close the session first" };
    vi.mocked(sessionDelete).mockRejectedValueOnce(cause);
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    await renderPanel(usage, [endedSession("session-build")]);
    const initial = container.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!initial) throw new Error("delete control did not render");
    await act(async () => initial.click());
    const confirm = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".history-delete-action"),
    ).find((button) => button.textContent === "Delete from history");
    if (!confirm) throw new Error("delete confirmation did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "The agent daemon refused that request as invalid.",
    );
  });

  it("resets delete confirmation after a failed delete", async () => {
    const cause = { code: "invalid_request", message: "close the session first" };
    vi.mocked(sessionDelete).mockRejectedValueOnce(cause);
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    await renderPanel(usage, [endedSession("session-build")]);
    const initial = container.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!initial) throw new Error("delete control did not render");
    await act(async () => initial.click());
    const confirm = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".history-delete-action"),
    ).find((button) => button.textContent === "Delete from history");
    if (!confirm) throw new Error("delete confirmation did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "The agent daemon refused that request as invalid.",
    );
    expect(container.querySelector<HTMLButtonElement>(".history-delete-action")?.textContent).toBe(
      "Delete",
    );
  });

  it("renders sessions when the session list fails and explains the degraded join", async () => {
    const cause = new Error("sessions unavailable");
    vi.mocked(journalUsage).mockResolvedValueOnce(baseUsage());
    vi.mocked(sessionsList).mockRejectedValueOnce(cause);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    expect(container.textContent).toContain("Build history");
    expect(container.textContent).toContain("Review history");
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "sessions unavailable",
    );
  });

  it("shows a load error without crashing", async () => {
    vi.mocked(journalUsage).mockRejectedValueOnce(new Error("history unavailable"));
    vi.mocked(sessionsList).mockResolvedValueOnce([]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("history unavailable");
  });

  it("renders an empty state", async () => {
    const usage = baseUsage();
    usage.perSession = [];
    usage.sessionCount = 0;
    await renderPanel(usage);
    expect(container.textContent).toContain("No saved history.");
  });

  it("declares when the oldest part of a transcript was removed", async () => {
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    const session = endedSession("session-build");
    session.state = {
      type: "ended",
      generation: 1,
      code: 0,
      integrity: { kind: "truncated", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 1 },
    };
    await renderPanel(usage, [session]);
    expect(container.textContent).toContain("Oldest part removed by the history limit.");
  });

  it("does not update state after unmount while usage is pending", async () => {
    let resolveUsage: ((value: JournalUsage) => void) | undefined;
    vi.mocked(journalUsage).mockReturnValueOnce(
      new Promise<JournalUsage>((resolve) => {
        resolveUsage = resolve;
      }),
    );
    vi.mocked(sessionsList).mockResolvedValueOnce([]);
    root = createRoot(container);
    await act(async () => root.render(<HistoryPanel now={now} search="" />));
    await act(async () => root.unmount());
    resolveUsage?.(baseUsage());
    await act(async () => undefined);
  });

  it("refetches usage and the session list after a successful delete", async () => {
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    vi.mocked(journalUsage).mockResolvedValue(usage);
    vi.mocked(sessionsList).mockResolvedValue([endedSession("session-build")]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    expect(journalUsage).toHaveBeenCalledTimes(1);
    expect(sessionsList).toHaveBeenCalledTimes(1);
    const initial = container.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!initial) throw new Error("delete control did not render");
    await act(async () => initial.click());
    const confirm = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".history-delete-action"),
    ).find((button) => button.textContent === "Delete from history");
    if (!confirm) throw new Error("delete confirmation did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);
    expect(sessionDelete).toHaveBeenCalledWith("session-build");
    expect(journalUsage).toHaveBeenCalledTimes(2);
    expect(sessionsList).toHaveBeenCalledTimes(2);
  });

  it("ticks relative times while the panel stays open and clears the interval on unmount", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], updatedAtMs: now }];
    vi.mocked(journalUsage).mockResolvedValue(usage);
    vi.mocked(sessionsList).mockResolvedValue([]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel search="" />);
      await Promise.resolve();
    });
    expect(container.textContent).toContain("just now");
    await act(async () => {
      vi.advanceTimersByTime(60_000);
    });
    expect(container.textContent).toContain("1m ago");
    const clearInterval = vi.spyOn(globalThis, "clearInterval");
    await act(async () => root.unmount());
    expect(clearInterval).toHaveBeenCalled();
    clearInterval.mockRestore();
    root = createRoot(container);
  });

  it("renders usage rows when sessionsList resolves undefined", async () => {
    vi.mocked(journalUsage).mockResolvedValueOnce(baseUsage());
    vi.mocked(sessionsList).mockResolvedValueOnce(undefined as unknown as Session[]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    expect(container.textContent).toContain("Build history");
  });

  it("ignores a second delete click while a delete is in flight", async () => {
    let resolveDelete: (() => void) | undefined;
    vi.mocked(sessionDelete).mockReturnValue(
      new Promise<void>((resolve) => {
        resolveDelete = resolve;
      }),
    );
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    await renderPanel(usage, [endedSession("session-build")]);
    const initial = container.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!initial) throw new Error("delete control did not render");
    await act(async () => initial.click());
    const confirm = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".history-delete-action"),
    ).find((button) => button.textContent === "Delete from history");
    if (!confirm) throw new Error("delete confirmation did not render");
    await act(async () => confirm.click());
    expect(sessionDelete).toHaveBeenCalledTimes(1);
    const pending = container.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!pending) throw new Error("pending delete control did not render");
    await act(async () => pending.click());
    expect(sessionDelete).toHaveBeenCalledTimes(1);
    await act(async () => resolveDelete?.());
  });

  it("does not refresh usage or sessions when delete resolves after unmount", async () => {
    let resolveDelete: (() => void) | undefined;
    vi.mocked(sessionDelete).mockReturnValue(
      new Promise<void>((resolve) => {
        resolveDelete = resolve;
      }),
    );
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    await renderPanel(usage, [endedSession("session-build")]);
    expect(journalUsage).toHaveBeenCalledTimes(1);
    expect(sessionsList).toHaveBeenCalledTimes(1);
    const initial = container.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!initial) throw new Error("delete control did not render");
    await act(async () => initial.click());
    const confirm = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".history-delete-action"),
    ).find((button) => button.textContent === "Delete from history");
    if (!confirm) throw new Error("delete confirmation did not render");
    await act(async () => confirm.click());
    expect(sessionDelete).toHaveBeenCalledTimes(1);
    expect(journalUsage).toHaveBeenCalledTimes(1);
    expect(sessionsList).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
    resolveDelete?.();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    expect(journalUsage).toHaveBeenCalledTimes(1);
    expect(sessionsList).toHaveBeenCalledTimes(1);
    root = createRoot(container);
  });
});
