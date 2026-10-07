// @vitest-environment happy-dom

// Session navigation stands a tool tab down: creating a session or opening
// one from History must show that session, never leave the tool tab over it.

const pruneCalls: string[] = [];

vi.mock("./strip/toolTabs", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./strip/toolTabs")>();
  return {
    ...actual,
    pruneToolTabsForWorkspaces: (
      tabs: Parameters<typeof actual.pruneToolTabsForWorkspaces>[0],
      known: Parameters<typeof actual.pruneToolTabsForWorkspaces>[1],
    ) => {
      pruneCalls.push([...known].sort().join(","));
      return actual.pruneToolTabsForWorkspaces(tabs, known);
    },
  };
});

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  flush,
  liveSnapshot,
  plainClick,
  pushSnapshots,
  renderWorkspace,
  tabElement,
  terminalSession,
} from "./bulkCloseHarness";
import {
  journalUsage,
  sessionCreate,
  sessionResume,
  sessionsList,
  workspaceGitStatus,
} from "../../lib/tauri";
import { toolTabId } from "./strip/toolTabs";
import { sharedSessionController } from "./workspaceSessions";
import { lookedAtSessionId } from "./presence";
import { fireAttentionToast, type ToastContent } from "./attentionNotice";

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await afterEachHarness();
});

function statusWithRow(path: string) {
  return {
    isGit: true,
    dirty: true,
    branch: "main",
    totals: { additions: 3, deletions: 1 },
    rows: [
      {
        path,
        renamedFrom: null,
        additions: 3,
        deletions: 1,
        status: "modified" as const,
        capped: false,
      },
    ],
    error: null,
  };
}

/** A diff tab left active, through the Changes pencil. */
async function openActiveDiffTab(): Promise<string> {
  vi.mocked(workspaceGitStatus).mockResolvedValue(statusWithRow("src/writer.ts"));
  await renderWorkspace();
  await act(async () => {
    document.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
  });
  await flush();
  await act(async () => {
    document.querySelector<HTMLButtonElement>('[aria-label="Open diff in a tab"]')?.click();
  });
  await flush();
  const id = toolTabId("diff", "workspace-1", "src/writer.ts");
  expect(tabElement(id).getAttribute("aria-selected")).toBe("true");
  return id;
}

function terminalSurfaces(): string[] {
  return [...document.querySelectorAll('[data-testid="terminal-surface"]')].map(
    (surface) => surface.textContent ?? "",
  );
}

async function chooseNewTabEntry(label: string): Promise<void> {
  await act(async () => {
    document.querySelector<HTMLButtonElement>('[aria-label="New tab"]')?.click();
  });
  await flush();
  const item = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
    (candidate) => candidate.textContent === label,
  );
  if (item === undefined) throw new Error(`+ menu item did not render: ${label}`);
  await act(async () => {
    item.click();
  });
  await flush();
}

describe("creating a session over an active tool tab", () => {
  it("a new terminal mounts its surface and takes the selection", async () => {
    const toolId = await openActiveDiffTab();
    vi.mocked(sessionCreate).mockResolvedValue(terminalSession("session-4", "fresh shell"));

    await chooseNewTabEntry("Terminal");
    await flush();
    await flush();

    expect(tabElement(toolId).getAttribute("aria-selected")).toBe("false");
    expect(tabElement("session-4").getAttribute("aria-selected")).toBe("true");
    expect(terminalSurfaces()).toContain("session-4");
  });

  it("a create that fails leaves the tool tab standing", async () => {
    const toolId = await openActiveDiffTab();
    vi.mocked(sessionCreate).mockRejectedValueOnce(new Error("daemon refused"));

    await chooseNewTabEntry("Terminal");
    await flush();
    await flush();

    expect(tabElement(toolId).getAttribute("aria-selected")).toBe("true");
    expect(terminalSurfaces()).not.toContain("session-4");
  });
});

describe("roster pushes", () => {
  it("a push with unchanged workspaces never runs the workspace prune", async () => {
    await renderWorkspace();
    await flush();
    pruneCalls.length = 0;

    await pushSnapshots([
      liveSnapshot("agent-one", "Agent one", "acp"),
      liveSnapshot("session-2", "shell two"),
      liveSnapshot("session-3", "shell three"),
    ]);
    await flush();
    await flush();
    // Keyed on the workspace-id set's contents, not the Set's identity:
    // the session-facts cascade below mints a new Set on every push, and an
    // identity check would fire this prune — a second full root pass whose
    // abandoned output never even reaches the strip — on the hottest path.
    expect(pruneCalls).toEqual([]);
  });
});

describe("opening History over an active tool tab", () => {
  it("a reopened agent takes the selection from the active tool tab", async () => {
    const toolId = await openActiveDiffTab();
    const saved = {
      ...terminalSession("saved-1", "saved one"),
      kind: "acp" as const,
      resumable: true,
    };
    vi.mocked(sessionsList).mockResolvedValue([
      terminalSession("session-2", "shell two"),
      terminalSession("session-3", "shell three"),
      saved,
    ]);
    vi.mocked(journalUsage).mockResolvedValue({
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
      perSession: [{ id: "saved-1", title: "saved one", kind: "acp", bytes: 32, updatedAtMs: 0 }],
    });
    vi.mocked(sessionResume).mockResolvedValue({ type: "resumed", session: saved });

    await act(async () => {
      document.querySelector<HTMLButtonElement>(".workspace-history-button")?.click();
    });
    await flush();
    await flush();
    const reopen = document.querySelector<HTMLButtonElement>(".history-reopen-action");
    if (reopen === null) throw new Error("history reopen button did not render");
    await act(async () => {
      reopen.click();
    });
    await flush();
    await flush();

    expect(tabElement(toolId).getAttribute("aria-selected")).toBe("false");
    expect(tabElement("saved-1").getAttribute("aria-selected")).toBe("true");
  });
});

describe("presence reporting", () => {
  it("keeps one controller subscription across tool-tab switches", async () => {
    const toolId = await openActiveDiffTab();
    const controller = sharedSessionController();
    let subscribes = 0;
    let unsubscribes = 0;
    const actualSubscribe = controller.subscribe.bind(controller);
    const spy = vi.spyOn(controller, "subscribe").mockImplementation(((listener: () => void) => {
      subscribes += 1;
      const unsubscribe = actualSubscribe(listener);
      return () => {
        unsubscribes += 1;
        unsubscribe();
      };
    }) as typeof controller.subscribe);
    try {
      await plainClick("session-2");
      await flush();
      await plainClick(toolId);
      await flush();
      // App-lifetime subscription: switching tabs reports through the ref,
      // never by tearing the controller subscription down and remaking it.
      expect(subscribes).toBe(0);
      expect(unsubscribes).toBe(0);
    } finally {
      spy.mockRestore();
    }
  });

  it("announces a raise on the session hidden behind a tool tab", async () => {
    await openActiveDiffTab();
    expect(lookedAtSessionId()).toBeNull();
    const sent: ToastContent[] = [];
    fireAttentionToast(
      { sessionId: "session-2", workspaceId: null },
      "shell two",
      { reason: "finished", atMs: Date.now() },
      {
        send: async (content) => {
          sent.push(content);
        },
        windowState: async () => ({ visible: true, focused: true, minimized: false }),
      },
    );
    await flush();
    await flush();
    await flush();
    expect(sent).toHaveLength(1);
    expect(sent[0]?.title).toContain("shell two");
  });
});
