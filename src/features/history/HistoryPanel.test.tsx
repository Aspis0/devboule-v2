// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { JournalUsage, ResumeResult, Session } from "../../types/ipc";
import type { WorkspaceProject } from "../workspace/workspaceProjects";
import * as historyGrouping from "./historyGrouping";
import * as sessionStateDisplay from "../workspace/sessionStateDisplay";

vi.mock("../../lib/tauri", () => ({
  isCommandError: vi.fn(
    (error: unknown) =>
      typeof error === "object" && error !== null && "code" in error && "message" in error,
  ),
  journalUsage: vi.fn(),
  sessionDelete: vi.fn(),
  sessionResume: vi.fn(),
  sessionsList: vi.fn(),
  workspaceGitStatus: vi.fn(async () => ({
    isGit: false,
    dirty: false,
    branch: null,
    totals: { additions: 0, deletions: 0 },
    rows: [],
    error: null,
  })),
}));

import {
  journalUsage,
  sessionDelete,
  sessionResume,
  sessionsList,
  workspaceGitStatus,
} from "../../lib/tauri";
import { HistoryPanel } from "./HistoryPanel";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const now = new Date(2026, 8, 4, 12, 0, 0, 0).getTime();

let container: HTMLDivElement;
let root: Root;

function endedSession(id: string): Session {
  return {
    id,
    workspaceId: "workspace-rust",
    kind: "acp",
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
        kind: "acp",
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

function renderPanel(
  usage: JournalUsage = baseUsage(),
  sessions: Session[] = [],
  search = "",
  projects: WorkspaceProject[] = [],
) {
  vi.mocked(journalUsage).mockResolvedValueOnce(usage);
  vi.mocked(sessionsList).mockResolvedValueOnce(sessions);
  root = createRoot(container);
  return act(async () => {
    root.render(<HistoryPanel now={now} search={search} projects={projects} />);
    await Promise.resolve();
  });
}

describe("HistoryPanel", () => {
  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    localStorage.removeItem("devboule.historyShowAll");
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
    vi.restoreAllMocks();
    vi.clearAllMocks();
  });

  it("renders session titles and groups them under day headings", async () => {
    await renderPanel();
    expect(container.textContent).toContain("Build history");
    expect(container.textContent).toContain("Review history");
    expect(container.textContent).toContain("Today");
    expect(container.textContent).toContain("Yesterday");
  });

  it("keeps each row to title and workspace/time lines, with bytes and reopen reason in its tooltip", async () => {
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: "session-build", bytes: 58_159 }];
    await renderPanel(usage, [{ ...endedSession("session-build"), resumable: false }]);
    const row = container.querySelector<HTMLElement>(".history-row");
    const activation = row?.querySelector<HTMLButtonElement>(".history-row-main");
    const copy = row?.querySelector<HTMLElement>(".history-row-copy");
    if (!row || !activation || !copy) throw new Error("History row did not render");
    expect(copy.querySelectorAll(":scope > *")).toHaveLength(2);
    expect(copy.textContent).not.toContain("bytes");
    expect(copy.textContent).not.toContain("Not reopenable");
    expect(activation.title).toContain("58 159 bytes");
    expect(activation.title).toContain("not resumable");
    expect(activation.getAttribute("aria-disabled")).toBe("true");
  });

  it("shows the journal totals in one quiet, human-readable line", async () => {
    const usage = baseUsage();
    usage.totalBytes = 17_928_791;
    usage.sessionCount = 163;
    await renderPanel(usage);
    const summary = container.querySelector<HTMLElement>(".history-usage");
    expect(summary?.textContent).toBe("163 saved sessions · 17.9 MB");
  });

  it("keeps the relative day count aligned with the row's calendar-day heading", async () => {
    const octoberFirst = new Date(2026, 9, 1, 1, 0, 0, 0).getTime();
    const septemberTwentyNinth = new Date(2026, 8, 29, 23, 0, 0, 0).getTime();
    const usage = baseUsage();
    usage.perSession = [
      { ...usage.perSession[0], id: "session-build", updatedAtMs: septemberTwentyNinth },
    ];
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={octoberFirst} search="" />);
      await Promise.resolve();
    });
    const group = container.querySelector<HTMLElement>(".history-day-group");
    const meta = container.querySelector<HTMLElement>(".history-row-meta");
    expect(group?.querySelector(".history-day-heading")?.textContent).toBe("29 Sep 2026");
    expect(meta?.textContent).toContain("2d ago");
  });

  it("uses one date-grouped list for journaled and running or recovered top-level agents only", async () => {
    const live = {
      ...resumableSession("agent-live"),
      title: "Live agent",
      state: { type: "live" as const, generation: 1 },
      createdAtMs: now,
    };
    const silent = {
      ...resumableSession("agent-silent"),
      title: "Silent agent",
      state: { type: "silent" as const, generation: 1 },
      createdAtMs: now - 86_400_000,
    };
    const recovered = {
      ...resumableSession("agent-recovered"),
      title: "Recovered agent",
      state: {
        type: "recovered" as const,
        generation: 1,
        integrity: {
          kind: "unverifiable" as const,
          droppedFrames: 0,
          droppedBytes: 0,
          trimmedBytes: 0,
        },
      },
      elapsedMs: null,
      createdAtMs: now,
    };
    const child = { ...live, id: "agent-child", title: "Child agent", createdBy: live.id };
    const legacyChild = {
      ...live,
      id: "agent-legacy-child",
      title: "Legacy child",
      contextId: live.id,
    };
    const terminal = { ...live, id: "terminal-live", kind: "terminal" as const, title: "Terminal" };
    const ended = {
      ...live,
      id: "agent-ended",
      title: "Ended agent",
      state: {
        type: "ended" as const,
        generation: 1,
        code: 0,
        integrity: { kind: "complete" as const },
      },
    };
    const usage = baseUsage();
    usage.perSession = [
      { ...usage.perSession[0], id: "agent-live", title: "Live agent" },
      { ...usage.perSession[0], id: "agent-silent", title: "Silent agent" },
    ];
    await renderPanel(usage, [live, silent, recovered, child, legacyChild, ended, terminal]);
    expect(container.querySelectorAll(".history-row-main")).toHaveLength(3);
    expect(container.textContent).toContain("Live agent");
    expect(container.textContent).toContain("Silent agent");
    expect(container.textContent).toContain("Recovered agent");
    expect(container.textContent).not.toContain("Child agent");
    expect(container.textContent).not.toContain("Legacy child");
    expect(container.textContent).not.toContain("Ended agent");
    expect(container.textContent).not.toContain("Unknown date");
  });

  it("resolves workspace names, omits ids and placeholders, and filters with History search", async () => {
    const live = {
      ...resumableSession("agent-live"),
      title: "Live agent",
      state: { type: "live" as const, generation: 1 },
      createdAtMs: now,
    };
    const projects: WorkspaceProject[] = [
      {
        id: "project-rust",
        name: "Rust project",
        path: "C:\\rust",
        workspaces: [
          {
            id: "workspace-rust",
            projectId: "project-rust",
            title: "Rust workspace",
            displayTitle: "Rust workspace",
            isolation: "local",
            path: "C:\\rust",
            meta: null,
            stateDot: null,
          },
        ],
      },
    ];
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: live.id, title: "Live agent" }];
    await renderPanel(usage, [live], "Rust workspace", projects);
    expect(container.querySelector(".history-row-main")?.getAttribute("aria-label")).toContain(
      "Live agent, Rust workspace",
    );
    expect(container.textContent).not.toContain("workspace-rust");
    expect(container.textContent).not.toContain("—");
    await act(async () =>
      root.render(<HistoryPanel now={now} search="nothing" projects={projects} />),
    );
    expect(container.querySelector(".history-day-group")).toBeNull();
    expect(container.querySelector(".history-row-main")).toBeNull();
  });

  it("labels a session whose workspace is gone as a deleted workspace", async () => {
    const projects: WorkspaceProject[] = [
      {
        id: "project-rust",
        name: "Rust project",
        path: "C:\\rust",
        workspaces: [
          {
            id: "workspace-rust",
            projectId: "project-rust",
            title: "Rust workspace",
            displayTitle: "Rust workspace",
            isolation: "local",
            path: "C:\\rust",
            meta: null,
            stateDot: null,
          },
        ],
      },
    ];
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: "session-gone", title: "Kept session" }];
    const kept = { ...endedSession("session-gone"), workspaceId: "workspace-deleted" };
    await renderPanel(usage, [kept], "", projects);

    const anchor = container.querySelector('[data-agent-id="session-gone"]');
    expect(anchor?.getAttribute("aria-label")).toContain("Deleted workspace");
    expect(container.textContent).toContain("Deleted workspace");
    expect(container.textContent).not.toContain("workspace-deleted");
  });

  it("claims no deletion before any project has answered", async () => {
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: "session-gone", title: "Kept session" }];
    const kept = { ...endedSession("session-gone"), workspaceId: "workspace-deleted" };
    await renderPanel(usage, [kept]);

    expect(
      container.querySelector('[data-agent-id="session-gone"]')?.getAttribute("aria-label"),
    ).not.toContain("Deleted workspace");
  });

  it("claims no deletion while any project's workspace list has failed", async () => {
    const projects: WorkspaceProject[] = [
      {
        id: "project-rust",
        name: "Rust project",
        path: "C:\\rust",
        workspaces: [],
        workspaceError: { sentence: "The agent daemon is not responding.", detail: null },
      },
    ];
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: "session-gone", title: "Kept session" }];
    const kept = { ...endedSession("session-gone"), workspaceId: "workspace-unknown" };
    await renderPanel(usage, [kept], "", projects);

    expect(
      container.querySelector('[data-agent-id="session-gone"]')?.getAttribute("aria-label"),
    ).not.toContain("Deleted workspace");
  });

  it("renders a journaled agent once when it is also present in the roster", async () => {
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: "session-build", title: "Journal agent" }];
    const agent = {
      ...resumableSession("session-build"),
      title: "Journal agent",
      state: { type: "live" as const, generation: 1 },
      createdAtMs: now,
    };
    const other = { ...agent, id: "agent-other", title: "Other agent" };
    await renderPanel(usage, [agent, other]);
    expect(container.querySelectorAll(".history-row-main")).toHaveLength(2);
    expect(container.textContent).toContain("Other agent");
    expect(container.querySelectorAll(".history-row")).toHaveLength(2);
    expect(container.textContent?.match(/Journal agent/g)).toHaveLength(1);
    const row = container.querySelector<HTMLButtonElement>('[data-agent-id="session-build"]');
    expect(row?.parentElement?.querySelector(".history-row-copy")?.textContent).not.toContain(
      "400 bytes",
    );
    expect(row?.title).toContain("400 bytes");
  });

  it("filters the unified agent list with History search", async () => {
    const live = {
      ...resumableSession("agent-alpha"),
      title: "Alpha agent",
      displayName: "Alpha",
      state: { type: "live" as const, generation: 1 },
      createdAtMs: now,
    };
    await renderPanel(baseUsage(), [live], "alpha");
    expect(container.querySelectorAll(".history-row-main")).toHaveLength(1);
    await act(async () => root.render(<HistoryPanel now={now} search="review" />));
    expect(container.querySelectorAll(".history-row-main")).toHaveLength(1);
    expect(container.textContent).toContain("Review history");
  });

  it("searches branch names on the same History list", async () => {
    const agent = {
      ...resumableSession("agent-branch"),
      title: "Build",
      workspaceId: "workspace-rust",
      createdAtMs: now,
    };
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: agent.id, title: agent.title }];
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([agent]);
    root = createRoot(container);
    await act(async () => {
      root.render(
        <HistoryPanel
          now={now}
          search="feature/branches"
          branches={new Map([["workspace-rust", "feature/branches"]])}
        />,
      );
      await Promise.resolve();
    });
    await act(async () => undefined);
    expect(container.querySelectorAll(".history-row-main")).toHaveLength(1);
  });

  it("renders the branch passed in as a prop and issues no git status read", async () => {
    const agent = {
      ...resumableSession("agent-branch"),
      title: "Build",
      workspaceId: "workspace-rust",
      createdAtMs: now,
    };
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: agent.id, title: agent.title }];
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([agent]);
    root = createRoot(container);
    await act(async () => {
      root.render(
        <HistoryPanel now={now} search="" branches={new Map([["workspace-rust", "main"]])} />,
      );
      await Promise.resolve();
    });
    await act(async () => undefined);
    expect(container.querySelectorAll(".history-row-main")).toHaveLength(1);
    expect(workspaceGitStatus).not.toHaveBeenCalled();
  });

  it("keeps row grouping memoized when only selection changes", async () => {
    const projects: WorkspaceProject[] = [];
    await renderPanel(baseUsage(), [], "", projects);
    await act(async () => undefined);
    const group = vi.spyOn(historyGrouping, "groupByDay");
    const calls = group.mock.calls.length;
    await act(async () =>
      root.render(
        <HistoryPanel now={now} search="" projects={projects} selectedSessionId="session-review" />,
      ),
    );
    expect(group).toHaveBeenCalledTimes(calls);
  });

  it("opens live rows directly and marks selection without visible Selected copy", async () => {
    const onReopen = vi.fn();
    const live = {
      ...resumableSession("agent-live"),
      title: "Live agent",
      state: { type: "live" as const, generation: 1 },
      createdAtMs: now,
    };
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: live.id, title: "Live agent" }];
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([live]);
    root = createRoot(container);
    await act(async () => {
      root.render(
        <HistoryPanel now={now} search="" onReopen={onReopen} selectedSessionId={live.id} />,
      );
      await Promise.resolve();
    });
    const row = container.querySelector<HTMLButtonElement>(".history-row-main");
    if (!row) throw new Error("agent row did not render");
    expect(row.getAttribute("aria-current")).toBe("true");
    expect(row.getAttribute("aria-label")).toContain("Live agent");
    expect(row.parentElement?.querySelector(".history-row-copy")?.textContent).not.toContain(
      "Selected",
    );
    await act(async () => row.click());
    expect(onReopen).toHaveBeenCalledExactlyOnceWith(live);
    expect(sessionResume).not.toHaveBeenCalled();
  });

  it("activates ended rows through Reopen and offers Delete in the context menu", async () => {
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: "session-build", kind: "acp" }];
    await renderPanel(usage, [resumableSession("session-build")]);
    const row = container.querySelector<HTMLButtonElement>(".history-row-main");
    if (!row) throw new Error("History row did not render");
    await act(async () => row.click());
    expect(sessionResume).toHaveBeenCalledWith("session-build");
    await act(async () => {
      row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    });
    expect(container.querySelector('[role="menu"]')).not.toBeNull();
    expect(container.querySelector('[role="menuitem"]')?.textContent).toContain("Reopen");
    expect(container.textContent).toContain("Delete");
  });

  it("opens a recovered row through the existing workspace reopen path", async () => {
    const recovered: Session = {
      ...resumableSession("agent-recovered-click"),
      state: {
        type: "recovered",
        generation: 1,
        integrity: {
          kind: "unverifiable",
          droppedFrames: 0,
          droppedBytes: 0,
          trimmedBytes: 0,
        },
      },
      elapsedMs: null,
    };
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: recovered.id, title: "Recovered click" }];
    const onReopenAgent = vi.fn();
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([recovered]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" onReopenAgent={onReopenAgent} />);
      await Promise.resolve();
    });
    const row = container.querySelector<HTMLButtonElement>(
      '[data-agent-id="agent-recovered-click"]',
    );
    if (!row) throw new Error("recovered History row did not render");
    await act(async () => row.click());
    expect(onReopenAgent).toHaveBeenCalledExactlyOnceWith(recovered);
    expect(sessionResume).not.toHaveBeenCalled();
  });

  it("does not activate an ended nonresumable row", async () => {
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: "session-build", kind: "acp" }];
    const session = { ...endedSession("session-build"), resumable: false };
    const onReopen = vi.fn();
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([session]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" onReopen={onReopen} />);
      await Promise.resolve();
    });
    const row = container.querySelector<HTMLButtonElement>(".history-row-main");
    if (!row) throw new Error("History row did not render");
    expect(row.parentElement?.querySelector(".history-row-copy")?.textContent).not.toContain(
      "Not reopenable",
    );
    expect(row.title).toContain("not resumable");
    await act(async () => row.click());
    expect(sessionResume).not.toHaveBeenCalled();
    expect(onReopen).not.toHaveBeenCalled();
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

  it("shows a quiet saved-session count and human-readable size", async () => {
    await renderPanel();
    expect(container.querySelector(".history-usage")?.textContent).toBe(
      "2 saved sessions · 12.3 KB",
    );
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
    const button = container.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!button) throw new Error("delete control did not render");
    expect(button.textContent).toBe("Delete");
    expect(button.getAttribute("aria-disabled")).toBe("true");
    expect(button.hasAttribute("aria-label")).toBe(false);
    const describedBy = button.getAttribute("aria-describedby");
    if (!describedBy) throw new Error("running delete has no describedby");
    expect(document.getElementById(describedBy)?.textContent).toBe(
      "Archive the session before deleting it from history.",
    );
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
    ).find((button) => button.textContent === "Confirm");
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
    ).find((button) => button.textContent === "Confirm");
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
    ).find((button) => button.textContent === "Confirm");
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

  it("keeps the journal read's raw text as the alert's demoted detail", async () => {
    // The rejection passes through to useTrackedRequest unmolested, so the
    // mapped sentence renders and the daemon's own words stay reachable.
    vi.mocked(journalUsage).mockRejectedValueOnce({
      code: "journal",
      message: "journal is corrupt: unexpected tail",
    });
    vi.mocked(sessionsList).mockResolvedValueOnce([]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("Saved history could not be read or written.");
    expect(alert?.querySelector(".error-detail-sr-only")?.textContent).toBe(
      "journal is corrupt: unexpected tail",
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
    expect(container.textContent).toContain("No agents in History.");
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
    const row = container.querySelector<HTMLButtonElement>('[data-agent-id="session-build"]');
    expect(row?.textContent).not.toContain("Oldest part removed");
    expect(row?.title).toContain("Oldest part removed by the history limit.");
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
    ).find((button) => button.textContent === "Confirm");
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
    ).find((button) => button.textContent === "Confirm");
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
    ).find((button) => button.textContent === "Confirm");
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

  it("lists a silent agent that has a journal row, and activates it as running", async () => {
    const silent = {
      ...resumableSession("agent-silent"),
      title: "Silent agent",
      state: { type: "silent" as const, generation: 1 },
      createdAtMs: now,
    };
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: silent.id, title: "Silent agent" }];
    const onReopenAgent = vi.fn();
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([silent]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" onReopenAgent={onReopenAgent} />);
      await Promise.resolve();
    });
    const row = container.querySelector<HTMLButtonElement>('[data-agent-id="agent-silent"]');
    if (!row) throw new Error("silent History row did not render");
    expect(row.hasAttribute("disabled")).toBe(false);
    await act(async () => row.click());
    expect(onReopenAgent).toHaveBeenCalledExactlyOnceWith(silent);
    expect(sessionResume).not.toHaveBeenCalled();
  });

  it("guards a silent row's delete with the archive-first explanation", async () => {
    const silent = {
      ...endedSession("session-build"),
      state: { type: "silent" as const, generation: 1 },
    };
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    await renderPanel(usage, [silent]);
    const button = container.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!button) throw new Error("delete control did not render");
    expect(button.textContent).toBe("Delete");
    expect(button.getAttribute("aria-disabled")).toBe("true");
    expect(button.hasAttribute("aria-label")).toBe(false);
    const describedBy = button.getAttribute("aria-describedby");
    if (!describedBy) throw new Error("running delete has no describedby");
    expect(document.getElementById(describedBy)?.textContent).toBe(
      "Archive the session before deleting it from history.",
    );
  });

  it("hides terminals and subagents by default, and lists them with the toggle on", async () => {
    const parent = {
      ...resumableSession("agent-parent"),
      title: "Parent agent",
      state: { type: "live" as const, generation: 1 },
      createdAtMs: now,
    };
    const child = {
      ...parent,
      id: "agent-child",
      title: "Child agent",
      createdBy: parent.id,
    };
    const usage = baseUsage();
    usage.perSession = [
      { ...usage.perSession[0], id: parent.id, title: "Parent agent" },
      { ...usage.perSession[0], id: child.id, title: "Child agent" },
      { ...usage.perSession[0], id: "term-1", title: "Shell", kind: "terminal" as const },
    ];
    await renderPanel(usage, [parent, child]);
    expect(container.querySelectorAll(".history-row-main")).toHaveLength(1);
    expect(container.textContent).toContain("Parent agent");
    expect(container.textContent).not.toContain("Child agent");
    expect(container.textContent).not.toContain("Shell");
    const toggle = container.querySelector<HTMLInputElement>('input[type="checkbox"]');
    if (!toggle) throw new Error("History show-all toggle did not render");
    await act(async () => toggle.click());
    expect(container.querySelectorAll(".history-row-main")).toHaveLength(3);
    expect(container.textContent).toContain("Child agent");
    expect(container.textContent).toContain("Shell");
  });

  it("withholds rows whose top-level state is unknown while the roster is pending", async () => {
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: "agent-child", title: "Child agent" }];
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockReturnValueOnce(new Promise<Session[]>(() => {}));
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    expect(container.textContent).not.toContain("Child agent");
    expect(container.textContent).toContain("Loading history");
  });

  it("states the failure truthfully when the roster read fails", async () => {
    vi.mocked(journalUsage).mockResolvedValueOnce(baseUsage());
    vi.mocked(sessionsList).mockRejectedValueOnce(new Error("sessions unavailable"));
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    expect(container.textContent).toContain("Build history");
    expect(container.textContent).toContain("Session details are unavailable");
    expect(container.textContent).toContain("this list is unfiltered");
    expect(container.textContent).toContain("rows show no workspace");
    const toggle = container.querySelector<HTMLInputElement>('input[type="checkbox"]');
    if (!toggle) throw new Error("History show-all toggle did not render");
    await act(async () => toggle.click());
    expect(container.textContent).not.toContain("this list is unfiltered");
    expect(container.textContent).toContain("rows show no workspace");
  });

  it("groups a push-only running row under Today instead of Unknown date", async () => {
    const live = {
      ...resumableSession("agent-push"),
      title: "Push agent",
      state: { type: "live" as const, generation: 1 },
      createdAtMs: undefined,
    };
    const usage = baseUsage();
    usage.perSession = [];
    usage.sessionCount = 0;
    usage.totalBytes = 0;
    await renderPanel(usage, [live]);
    expect(container.textContent).toContain("Push agent");
    expect(container.textContent).not.toContain("Unknown date");
    expect(container.querySelectorAll(".history-day-group")).toHaveLength(1);
    expect(container.querySelector(".history-day-group .history-day-heading")?.textContent).toBe(
      "Today",
    );
  });

  it("opens the context menu from a non-reopenable row with the keyboard", async () => {
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    await renderPanel(usage, [{ ...endedSession("session-build"), resumable: false }]);
    const row = container.querySelector<HTMLButtonElement>(".history-row-main");
    if (!row) throw new Error("History row did not render");
    expect(row.hasAttribute("disabled")).toBe(false);
    row.focus();
    expect(document.activeElement).toBe(row);
    await act(async () => {
      row.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "F10",
          shiftKey: true,
          bubbles: true,
          cancelable: true,
        }),
      );
    });
    expect(container.querySelector('[role="menu"]')).not.toBeNull();
  });

  it("focuses the menu on open, and Escape closes it back on the row", async () => {
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    await renderPanel(usage, [resumableSession("session-build")]);
    const row = container.querySelector<HTMLButtonElement>(".history-row-main");
    if (!row) throw new Error("History row did not render");
    await act(async () => {
      row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    });
    const menu = container.querySelector<HTMLElement>('[role="menu"]');
    if (!menu) throw new Error("History menu did not render");
    const items = [...menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
    const firstEnabled = items.find((item) => !item.disabled);
    if (!firstEnabled) throw new Error("History menu has no enabled item");
    expect(document.activeElement).toBe(firstEnabled);
    await act(async () => {
      menu.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(container.querySelector('[role="menu"]')).toBeNull();
    expect(document.activeElement).toBe(row);
  });

  it("moves menu focus with arrow keys", async () => {
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    await renderPanel(usage, [resumableSession("session-build")]);
    const row = container.querySelector<HTMLButtonElement>(".history-row-main");
    if (!row) throw new Error("History row did not render");
    await act(async () => {
      row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    });
    const menu = container.querySelector<HTMLElement>('[role="menu"]');
    if (!menu) throw new Error("History menu did not render");
    const items = [...menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].filter(
      (item) => !item.disabled,
    );
    if (items.length < 2) throw new Error("History menu needs two enabled items");
    expect(document.activeElement).toBe(items[0]);
    await act(async () => {
      menu.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });
    expect(document.activeElement).toBe(items[1]);
    await act(async () => {
      menu.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }));
    });
    expect(document.activeElement).toBe(items[0]);
  });
});

describe("HistoryPanel show-all, delete focus and row memo", () => {
  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    localStorage.removeItem("devboule.historyShowAll");
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
    localStorage.removeItem("devboule.historyShowAll");
    vi.restoreAllMocks();
    vi.clearAllMocks();
  });

  it("yields the fallback to a late roster and retries on reopen", async () => {
    vi.useFakeTimers();
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: "agent-known", title: "Known agent" }];
    let resolveRoster: ((sessions: Session[]) => void) | undefined;
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockReturnValueOnce(
      new Promise<Session[]>((resolve) => {
        resolveRoster = resolve;
      }),
    );
    const known = {
      ...resumableSession("agent-known"),
      title: "Known agent",
      state: { type: "live" as const, generation: 1 },
      createdAtMs: now,
    };
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    expect(container.textContent).toContain("Loading history");
    expect(container.textContent).not.toContain("Known agent");
    await act(async () => {
      vi.advanceTimersByTime(4999);
    });
    expect(container.textContent).toContain("Loading history");
    await act(async () => {
      vi.advanceTimersByTime(1);
    });
    expect(container.textContent).toContain("Known agent");
    expect(container.textContent).toContain("Waiting for session details");
    await act(async () => {
      resolveRoster?.([known]);
      vi.advanceTimersByTime(2000);
    });
    await act(async () => undefined);
    expect(container.textContent).toContain("Known agent");
    expect(container.textContent).not.toContain("Waiting for session details");
    await act(async () => root.unmount());
    root = createRoot(container);
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([known]);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    expect(container.textContent).toContain("Known agent");
    expect(vi.mocked(sessionsList).mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it("persists the show-all toggle across opens", async () => {
    const usage = baseUsage();
    usage.perSession = [
      { ...usage.perSession[0], id: "agent-parent", title: "Parent agent" },
      { ...usage.perSession[0], id: "term-1", title: "Shell", kind: "terminal" as const },
    ];
    vi.mocked(journalUsage).mockResolvedValue(usage);
    vi.mocked(sessionsList).mockResolvedValue([]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    expect(container.textContent).not.toContain("Shell");
    const toggle = container.querySelector<HTMLInputElement>('input[type="checkbox"]');
    if (!toggle) throw new Error("History show-all toggle did not render");
    await act(async () => toggle.click());
    expect(container.textContent).toContain("Shell");
    await act(async () => root.unmount());
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    expect(container.querySelector<HTMLInputElement>('input[type="checkbox"]')?.checked).toBe(true);
    expect(container.textContent).toContain("Shell");
  });

  it("deletes a subagent row reached through the toggle", async () => {
    const parent = {
      ...resumableSession("agent-parent"),
      title: "Parent agent",
      state: { type: "live" as const, generation: 1 },
      createdAtMs: now,
    };
    const child = { ...endedSession("agent-child"), createdBy: parent.id };
    const usage = baseUsage();
    usage.perSession = [
      { ...usage.perSession[0], id: parent.id, title: "Parent agent" },
      { ...usage.perSession[0], id: child.id, title: "Child agent" },
    ];
    vi.mocked(journalUsage).mockResolvedValue(usage);
    vi.mocked(sessionsList).mockResolvedValue([parent, child]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    expect(container.textContent).not.toContain("Child agent");
    const toggle = container.querySelector<HTMLInputElement>('input[type="checkbox"]');
    if (!toggle) throw new Error("History show-all toggle did not render");
    await act(async () => toggle.click());
    const childRow = container.querySelector<HTMLElement>('[data-agent-id="agent-child"]');
    if (!childRow) throw new Error("toggled subagent row did not render");
    const initial =
      childRow.parentElement?.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!initial) throw new Error("subagent delete control did not render");
    await act(async () => initial.click());
    const confirm =
      childRow.parentElement?.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!confirm || confirm.textContent !== "Confirm")
      throw new Error("subagent delete confirmation did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);
    expect(sessionDelete).toHaveBeenCalledWith("agent-child");
  });

  it("groups a push-only row under Today with its state word and no invented age", async () => {
    const live = {
      ...resumableSession("agent-push"),
      title: "Push agent",
      workspaceId: null,
      state: { type: "live" as const, generation: 1 },
      createdAtMs: undefined,
    };
    const usage = baseUsage();
    usage.perSession = [];
    usage.sessionCount = 0;
    usage.totalBytes = 0;
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([live]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    expect(container.querySelector(".history-day-group .history-day-heading")?.textContent).toBe(
      "Today",
    );
    const meta = container.querySelector<HTMLElement>(".history-row-meta");
    expect(meta?.textContent).toContain("Running");
    expect(meta?.textContent).not.toMatch(/just now|ago/);
  });

  it("moves focus to the next row after a delete", async () => {
    const usage = baseUsage();
    usage.perSession = [
      { ...usage.perSession[0], id: "gone", title: "Gone" },
      { ...usage.perSession[1], id: "stays", title: "Stays" },
    ];
    const remaining: JournalUsage = {
      ...usage,
      perSession: [usage.perSession[1]],
      sessionCount: 1,
    };
    vi.mocked(journalUsage).mockResolvedValueOnce(usage).mockResolvedValue(remaining);
    vi.mocked(sessionsList).mockResolvedValue([]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    const goneRow = container.querySelector<HTMLElement>('[data-agent-id="gone"]');
    const initial =
      goneRow?.parentElement?.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!goneRow || !initial) throw new Error("doomed row did not render");
    await act(async () => initial.click());
    const confirm =
      goneRow.parentElement?.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!confirm) throw new Error("delete confirmation did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);
    expect(sessionDelete).toHaveBeenCalledWith("gone");
    expect(document.activeElement?.getAttribute("data-agent-id")).toBe("stays");
  });

  it("moves focus to the heading when a delete empties the list", async () => {
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: "last", title: "Last" }];
    const emptied: JournalUsage = { ...usage, perSession: [], sessionCount: 0, totalBytes: 0 };
    vi.mocked(journalUsage).mockResolvedValueOnce(usage).mockResolvedValue(emptied);
    vi.mocked(sessionsList).mockResolvedValue([]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    const row = container.querySelector<HTMLElement>('[data-agent-id="last"]');
    const initial = row?.parentElement?.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!row || !initial) throw new Error("last row did not render");
    await act(async () => initial.click());
    const confirm = row.parentElement?.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!confirm) throw new Error("delete confirmation did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);
    expect(sessionDelete).toHaveBeenCalledWith("last");
    expect(container.textContent).toContain("No agents in History.");
    expect(document.activeElement).toBe(container.querySelector(".history-heading-title"));
  });

  it("offers Open for a running row in its menu", async () => {
    const live = {
      ...resumableSession("agent-live"),
      title: "Live agent",
      state: { type: "live" as const, generation: 1 },
      createdAtMs: now,
    };
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: live.id, title: "Live agent" }];
    const onReopenAgent = vi.fn();
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([live]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" onReopenAgent={onReopenAgent} />);
      await Promise.resolve();
    });
    const row = container.querySelector<HTMLButtonElement>(".history-row-main");
    if (!row) throw new Error("History row did not render");
    await act(async () => {
      row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    });
    const menu = container.querySelector<HTMLElement>('[role="menu"]');
    if (!menu) throw new Error("History menu did not render");
    const first = menu.querySelector<HTMLButtonElement>('[role="menuitem"]');
    expect(first?.textContent).toBe("Open");
    expect(first?.disabled).toBe(false);
    await act(async () => first?.click());
    expect(onReopenAgent).toHaveBeenCalledExactlyOnceWith(live);
    expect(container.querySelector('[role="menu"]')).toBeNull();
  });

  it("keeps the menu open to confirm a delete", async () => {
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([resumableSession("session-build")]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    const row = container.querySelector<HTMLButtonElement>(".history-row-main");
    if (!row) throw new Error("History row did not render");
    await act(async () => {
      row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    });
    const menu = container.querySelector<HTMLElement>('[role="menu"]');
    if (!menu) throw new Error("History menu did not render");
    const item = [...menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(
      (entry) => entry.textContent === "Delete",
    );
    if (!item) throw new Error("menu Delete item did not render");
    await act(async () => item.click());
    expect(sessionDelete).not.toHaveBeenCalled();
    const stillOpen = container.querySelector<HTMLElement>('[role="menu"]');
    if (!stillOpen) throw new Error("menu closed on the arming click");
    const armed = [...stillOpen.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(
      (entry) => entry.textContent === "Delete from history",
    );
    if (!armed) throw new Error("menu Delete did not arm to Delete from history");
    expect(stillOpen.contains(document.activeElement)).toBe(true);
    await act(async () => armed.click());
    expect(sessionDelete).toHaveBeenCalledWith("session-build");
  });

  it("re-renders only rows whose props change", async () => {
    // rosterStateDisplay runs once per row render and nowhere else on this
    // surface, so its call delta counts row renders. A search keystroke
    // that keeps both rows must add none; selecting one row must add one;
    // deleting one row must not re-render the survivor. (A wall-clock tick
    // legitimately re-renders: `now` is a row prop because ages are live.)
    const endedA = { ...resumableSession("agent-a"), title: "Agent A" };
    const liveB = {
      ...resumableSession("agent-b"),
      title: "Agent B",
      state: { type: "live" as const, generation: 1 },
      createdAtMs: now,
    };
    const usage = baseUsage();
    usage.perSession = [
      { ...usage.perSession[0], id: "agent-a", title: "Agent A" },
      { ...usage.perSession[0], id: "agent-b", title: "Agent B" },
    ];
    const withoutA: JournalUsage = {
      ...usage,
      perSession: [usage.perSession[1]],
      sessionCount: 1,
    };
    vi.mocked(journalUsage).mockResolvedValueOnce(usage).mockResolvedValue(withoutA);
    vi.mocked(sessionsList).mockResolvedValueOnce([endedA, liveB]).mockResolvedValue([liveB]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="agent" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    expect(container.querySelectorAll(".history-row-main")).toHaveLength(2);
    const renders = vi.spyOn(sessionStateDisplay, "rosterStateDisplay");
    // The doomed row is ended, the survivor live: only live-state calls
    // count the survivor's renders, so its own arming churn stays out.
    const liveRenders = () =>
      renders.mock.calls.filter((call) => (call[0] as { type: string }).type === "live").length;
    try {
      // Same branch (flat list), same matches, different query text: the
      // filter re-runs but neither row may re-render. (Switching between
      // the grouped and flat layouts remounts rows by construction.)
      const base = liveRenders();
      await act(async () => {
        root.render(<HistoryPanel now={now} search="Agent" />);
      });
      await act(async () => undefined);
      expect(container.querySelectorAll(".history-row-main")).toHaveLength(2);
      expect(liveRenders()).toBe(base);
      await act(async () => {
        root.render(<HistoryPanel now={now} search="Agent" selectedSessionId="agent-b" />);
      });
      await act(async () => undefined);
      expect(liveRenders()).toBe(base + 1);
      const doomed = container.querySelector<HTMLElement>('[data-agent-id="agent-a"]');
      const initial =
        doomed?.parentElement?.querySelector<HTMLButtonElement>(".history-delete-action");
      if (!doomed || !initial) throw new Error("doomed row did not render");
      await act(async () => initial.click());
      const confirm =
        doomed.parentElement?.querySelector<HTMLButtonElement>(".history-delete-action");
      if (!confirm) throw new Error("delete confirmation did not render");
      await act(async () => confirm.click());
      await act(async () => undefined);
      expect(sessionDelete).toHaveBeenCalledWith("agent-a");
      expect(liveRenders()).toBe(base + 1);
      expect(document.activeElement?.getAttribute("data-agent-id")).toBe("agent-b");
    } finally {
      renders.mockRestore();
    }
  });

  it("keeps unchanged rows un-rendered when a roster read rebuilds every session", async () => {
    // A roster read returns new Session objects for every row. A failed
    // resume on A triggers one; C's state changes in it, B's does not.
    // Each live row's distinct elapsedMs tags its rosterStateDisplay calls.
    const endedA = { ...resumableSession("agent-a"), title: "Agent A" };
    const liveB: Session = {
      ...endedSession("agent-b"),
      state: { type: "live", generation: 1 },
      elapsedMs: 111,
    };
    const liveC: Session = {
      ...endedSession("agent-c"),
      state: { type: "live", generation: 1 },
      elapsedMs: 222,
    };
    const usage = baseUsage();
    usage.perSession = ["agent-a", "agent-b", "agent-c"].map((id) => ({
      ...usage.perSession[0],
      id,
      title: id,
    }));
    const reread = [
      { ...structuredClone(endedA), resumable: false },
      structuredClone(liveB),
      { ...structuredClone(liveC), state: { type: "silent" as const, generation: 1 } },
    ];
    vi.mocked(sessionResume).mockRejectedValueOnce(new Error("resume failed"));
    await renderPanel(usage, [endedA, liveB, liveC]);
    await act(async () => undefined);
    expect(container.querySelectorAll(".history-row-main")).toHaveLength(3);
    const renders = vi.spyOn(sessionStateDisplay, "rosterStateDisplay");
    const rendersOf = (elapsedMs: number) =>
      renders.mock.calls.filter((call) => call[1] === elapsedMs).length;
    vi.mocked(sessionsList).mockResolvedValueOnce(reread);
    try {
      const reopen = container.querySelector<HTMLButtonElement>(".history-reopen-action");
      if (!reopen) throw new Error("reopen control did not render");
      await act(async () => reopen.click());
      await act(async () => undefined);
      expect(sessionsList).toHaveBeenCalledTimes(2);
      expect(container.querySelector(".history-reopen-action")).toBeNull();
      expect(rendersOf(222)).toBeGreaterThan(0);
      expect(rendersOf(111)).toBe(0);
    } finally {
      renders.mockRestore();
      vi.mocked(sessionsList).mockReset();
    }
  });

  it("carries the full title on the row control that takes pointer events", async () => {
    const longTitle = "Work on the requested design change for the sidebar agent list panel";
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: "session-long", title: longTitle }];
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    const control = container.querySelector<HTMLButtonElement>(".history-row-main");
    expect(control?.getAttribute("title")).toContain(longTitle);
  });
});

describe("HistoryPanel delete refusal and focus during deletes", () => {
  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    localStorage.removeItem("devboule.historyShowAll");
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
    localStorage.removeItem("devboule.historyShowAll");
    vi.restoreAllMocks();
    vi.clearAllMocks();
  });

  it("holds focus on Confirm when a branch arrives through the prop mid-delete", async () => {
    const liveB = {
      ...resumableSession("agent-b"),
      title: "Agent B",
      state: { type: "live" as const, generation: 1 },
      createdAtMs: now,
    };
    const usage = baseUsage();
    usage.perSession = [
      { ...usage.perSession[0], id: "agent-a", title: "Agent A" },
      { ...usage.perSession[0], id: "agent-b", title: "Agent B" },
    ];
    const withoutA: JournalUsage = {
      ...usage,
      perSession: [usage.perSession[1]],
      sessionCount: 1,
    };
    let resolveDelete: (() => void) | undefined;
    vi.mocked(journalUsage).mockResolvedValueOnce(usage).mockResolvedValue(withoutA);
    vi.mocked(sessionsList)
      .mockResolvedValueOnce([{ ...endedSession("agent-a") }, liveB])
      .mockResolvedValue([liveB]);
    vi.mocked(sessionDelete).mockReturnValueOnce(
      new Promise<void>((resolve) => {
        resolveDelete = resolve;
      }),
    );
    const branchesOf = (branch: string) => new Map([["workspace-rust", branch]]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" branches={branchesOf("main")} />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    const doomed = container.querySelector<HTMLElement>('[data-agent-id="agent-a"]');
    const initial =
      doomed?.parentElement?.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!doomed || !initial) throw new Error("doomed row did not render");
    await act(async () => initial.click());
    const confirm =
      doomed.parentElement?.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!confirm) throw new Error("delete confirmation did not render");
    confirm.focus();
    await act(async () => confirm.click());
    await act(async () => undefined);
    expect(document.activeElement).toBe(confirm);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" branches={branchesOf("feature/x")} />);
    });
    await act(async () => undefined);
    expect(document.activeElement).toBe(confirm);
    await act(async () => resolveDelete?.());
    await act(async () => undefined);
    expect(sessionDelete).toHaveBeenCalledWith("agent-a");
    expect(document.activeElement?.getAttribute("data-agent-id")).toBe("agent-b");
    expect(workspaceGitStatus).not.toHaveBeenCalled();
  });

  it("keeps a running row's refused Delete focusable, so its reason is reachable", async () => {
    const live = {
      ...endedSession("session-build"),
      state: { type: "live" as const, generation: 1 },
    };
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([live]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    const row = container.querySelector<HTMLElement>('[data-agent-id="session-build"]');
    const button = row?.parentElement?.querySelector<HTMLButtonElement>(".history-delete-action");
    if (!button) throw new Error("running delete control did not render");
    expect(button.disabled).toBe(false);
    expect(button.getAttribute("aria-disabled")).toBe("true");
    act(() => button.focus());
    expect(document.activeElement).toBe(button);
    await act(async () => button.click());
    await act(async () => button.click());
    expect(button.textContent).toBe("Delete");
    expect(sessionDelete).not.toHaveBeenCalled();
  });

  it("leads the meta line with the marker when a workspace follows", async () => {
    const dead = { ...endedSession("agent-shut"), resumable: false };
    const usage = baseUsage();
    usage.perSession = [{ ...usage.perSession[0], id: "agent-shut", title: "Shut agent" }];
    const projects: WorkspaceProject[] = [
      {
        id: "project-rust",
        name: "Rust project",
        path: "C:\\rust",
        workspaces: [
          {
            id: "workspace-rust",
            projectId: "project-rust",
            title: "Rust workspace",
            displayTitle: "Rust workspace",
            isolation: "local",
            path: "C:\\rust",
            meta: null,
            stateDot: null,
          },
        ],
      },
    ];
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([dead]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" projects={projects} />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    const meta = container.querySelector<HTMLElement>(".history-row-meta");
    expect(meta?.textContent?.startsWith("Read-only · Rust workspace")).toBe(true);
  });

  it("reaches a running row's menu Delete by keyboard, hears why, and deletes nothing", async () => {
    const live = {
      ...endedSession("session-build"),
      state: { type: "live" as const, generation: 1 },
    };
    const usage = baseUsage();
    usage.perSession = [usage.perSession[0]];
    vi.mocked(journalUsage).mockResolvedValueOnce(usage);
    vi.mocked(sessionsList).mockResolvedValueOnce([live]);
    root = createRoot(container);
    await act(async () => {
      root.render(<HistoryPanel now={now} search="" />);
      await Promise.resolve();
    });
    await act(async () => undefined);
    const row = container.querySelector<HTMLButtonElement>(".history-row-main");
    if (!row) throw new Error("History row did not render");
    row.focus();
    await act(async () => {
      row.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "F10",
          shiftKey: true,
          bubbles: true,
          cancelable: true,
        }),
      );
    });
    const menu = container.querySelector<HTMLElement>('[role="menu"]');
    if (!menu) throw new Error("History menu did not render");
    const item = [...menu.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(
      (entry) => entry.textContent === "Delete",
    );
    if (!item) throw new Error("menu Delete item did not render");
    expect(item.disabled).toBe(false);
    expect(item.getAttribute("aria-disabled")).toBe("true");
    const describedBy = item.getAttribute("aria-describedby");
    if (!describedBy) throw new Error("menu Delete has no describedby");
    expect(document.getElementById(describedBy)?.textContent).toBe(
      "Archive the session before deleting it from history.",
    );
    await act(async () => {
      menu.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });
    expect(document.activeElement).toBe(item);
    // happy-dom runs no default activation: an unconsumed Enter or Space
    // would make the browser click the focused item, so the test dispatches
    // that click itself — both keys must reach the button unhandled, and
    // both must end in the same refusal.
    for (const key of ["Enter", " "]) {
      const down = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
      await act(async () => {
        item.dispatchEvent(down);
      });
      expect(down.defaultPrevented).toBe(false);
      await act(async () => item.click());
      expect(item.textContent).toBe("Delete");
      expect(sessionDelete).not.toHaveBeenCalled();
    }
    await act(async () => item.click());
    expect(item.textContent).toBe("Delete");
    expect(sessionDelete).not.toHaveBeenCalled();
  });
});
