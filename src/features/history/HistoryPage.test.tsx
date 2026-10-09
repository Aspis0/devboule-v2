// @vitest-environment happy-dom

// The History full page (slice sidebar-like-paseo): its own search field and
// host filter, rows grouped by day, one row per agent session reading
// "workspace › glyph title" with project, host, branch and relative time.
// The sidebar keeps the workspaces; that half is pinned in Workspace.test.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { JournalUsage, Session } from "../../types/ipc";
import type { WorkspaceProject } from "../workspace/workspaceProjects";
import { LOCAL_HOST_ID } from "../workspace/hosts/hostIdentity";

vi.mock("../../lib/tauri", () => ({
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

import { journalUsage, sessionsList } from "../../lib/tauri";
import { HistoryPanel } from "./HistoryPanel";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const now = new Date(2026, 8, 4, 12, 0, 0, 0).getTime();

let container: HTMLDivElement;
let root: Root;

function projects(): WorkspaceProject[] {
  return [
    {
      id: "project-1",
      name: "Alpha",
      path: "C:/code/alpha",
      hostId: LOCAL_HOST_ID,
      workspaces: [
        {
          id: "workspace-rust",
          projectId: "project-1",
          hostId: LOCAL_HOST_ID,
          title: "rust work",
          isolation: "local",
          path: "C:/code/alpha",
          displayTitle: "rust work",
          agents: { working: 0, waiting: 0 },
          elapsedMs: null,
          stateDot: null,
        },
      ],
    },
  ];
}

function usage(): JournalUsage {
  return {
    totalBytes: 400,
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
      { id: "session-build", title: "Build history", kind: "acp", bytes: 400, updatedAtMs: now },
    ],
  };
}

function liveSession(): Session {
  return {
    id: "session-build",
    workspaceId: "workspace-rust",
    kind: "acp",
    title: "Build history",
    createdAtMs: now,
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
  };
}

const HOSTS = [{ id: LOCAL_HOST_ID, name: "This PC" }];

async function renderPage(
  over: Partial<React.ComponentProps<typeof HistoryPanel>> = {},
): Promise<void> {
  vi.mocked(journalUsage).mockResolvedValueOnce(usage());
  vi.mocked(sessionsList).mockResolvedValueOnce([liveSession()]);
  root = createRoot(container);
  await act(async () => {
    root.render(
      <HistoryPanel
        now={now}
        search=""
        onSearchChange={() => {}}
        projects={projects()}
        branches={new Map([[`${LOCAL_HOST_ID}:workspace-rust` as never, "main"]])}
        hosts={HOSTS}
        hostFilter="all"
        onHostFilterChange={() => {}}
        {...over}
      />,
    );
    await Promise.resolve();
  });
}

describe("the History page", () => {
  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root?.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it("carries its own search field bound to the page search", async () => {
    const onSearchChange = vi.fn();
    await renderPage({ search: "Build", onSearchChange });

    const field = container.querySelector<HTMLInputElement>(".history-page-bar input");
    if (field === null) throw new Error("the page search field did not render");
    expect(field.value).toBe("Build");
    expect(field.getAttribute("aria-label")).toBe("Search history");
    await act(async () => {
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });
  });

  it("filters by host, starting from All hosts", async () => {
    const onHostFilterChange = vi.fn();
    await renderPage({ onHostFilterChange });

    const select = container.querySelector<HTMLSelectElement>(".history-page-bar select");
    if (select === null) throw new Error("the host filter did not render");
    expect(select.getAttribute("aria-label")).toBe("Host");
    expect([...select.options].map((option) => option.text)).toEqual(["All hosts", "This PC"]);
    expect(select.value).toBe("all");
    await act(async () => {
      select.value = LOCAL_HOST_ID;
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    expect(onHostFilterChange).toHaveBeenCalledWith(LOCAL_HOST_ID);
  });

  it("reads one row per session as workspace › glyph title, then project, host, branch, time", async () => {
    await renderPage();

    const row = container.querySelector<HTMLElement>(".history-row");
    if (row === null) throw new Error("the history row did not render");
    const first = row.querySelector<HTMLElement>(".history-row-title-line");
    const meta = row.querySelector<HTMLElement>(".history-row-meta");
    if (first === null || meta === null) throw new Error("the row lines did not render");
    expect(first.textContent).toContain("rust work");
    expect(first.textContent).toContain("›");
    expect(first.textContent).toContain("Build history");
    expect(first.querySelector("svg")).not.toBeNull();
    expect(meta.textContent).toContain("Alpha");
    expect(meta.textContent).toContain("This PC");
    expect(meta.textContent).toContain("main");
    expect(
      row.querySelector<HTMLButtonElement>(".history-row-main")?.getAttribute("aria-label"),
    ).toContain("This PC");
  });

  it("still groups rows by day", async () => {
    await renderPage();

    expect(container.querySelector(".history-day-heading")?.textContent).toBe("Today");
  });

  it("opens the session on row click", async () => {
    const onReopenAgent = vi.fn();
    await renderPage({ onReopenAgent });

    const activation = container.querySelector<HTMLButtonElement>(".history-row-main");
    if (activation === null) throw new Error("the row activation did not render");
    await act(async () => activation.click());
    expect(onReopenAgent).toHaveBeenCalledTimes(1);
  });
});
