// @vitest-environment happy-dom

// One host: the host-qualified identity must reach no DOM id, or the screen
// it names would differ from the one it replaced.

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  flush,
  plainClick,
  renderWorkspace,
  terminalSession,
} from "./bulkCloseHarness";
import {
  sessionsList,
  workspaceGitDiff,
  workspaceGitStatus,
  workspacesList,
} from "../../lib/tauri";
import { toolTabId } from "./strip/toolTabs";
import type { Session, Workspace as IpcWorkspace } from "../../types/ipc";

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await afterEachHarness();
});

const alpha: IpcWorkspace = {
  id: "workspace-1",
  projectId: "project-1",
  title: "alpha",
  isolation: "local",
  path: "C:\\devboule",
};
const beta: IpcWorkspace = {
  id: "workspace-2",
  projectId: "project-1",
  title: "beta",
  isolation: "local",
  path: "C:\\side",
};

function listedSessions(): Session[] {
  return [terminalSession("a-one", "A one"), terminalSession("a-two", "A two")];
}

function chipIds(): string[] {
  return [...document.querySelectorAll<HTMLElement>(".workspace-session-tab")].map(
    (chip) => chip.id,
  );
}

function selectedChips(): string[] {
  return [
    ...document.querySelectorAll<HTMLElement>('.workspace-session-tab[aria-selected="true"]'),
  ].map((chip) => chip.id);
}

function sidebarRow(title: string): HTMLButtonElement {
  const row = [...document.querySelectorAll<HTMLButtonElement>(".workspace-row")].find(
    (candidate) => candidate.textContent?.includes(title) === true,
  );
  if (row === undefined) throw new Error(`workspace row did not render: ${title}`);
  return row;
}

async function showWorkspace(title: string): Promise<void> {
  const row = sidebarRow(title);
  await act(async () => row.click());
  await flush();
}

function statusWithRow() {
  return {
    isGit: true,
    dirty: true,
    branch: "trunk",
    totals: { additions: 3, deletions: 1 },
    rows: [
      {
        path: "src/writer.ts",
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

function diffReply(text: string) {
  return {
    path: "src/writer.ts",
    isNew: false,
    isDeleted: false,
    additions: 1,
    deletions: 0,
    lines: [{ kind: "add" as const, text }],
    status: "ok" as const,
    error: null,
  };
}

async function openActiveDiffTab(): Promise<string> {
  await act(async () => {
    document.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
  });
  await flush();
  await act(async () => {
    document.querySelector<HTMLButtonElement>('[aria-label="Open diff in a tab"]')?.click();
  });
  await flush();
  return toolTabId("diff", "workspace-1", "src/writer.ts");
}

function sidePanelBody(): HTMLElement | null {
  return document.querySelector<HTMLElement>(".workspace-side-scroll");
}

describe("one host renders as it always did", () => {
  it("names every chip by its tab id alone, and selects the one it selected", async () => {
    vi.mocked(workspacesList).mockResolvedValue([alpha, beta]);
    vi.mocked(sessionsList).mockResolvedValue(listedSessions());
    vi.mocked(workspaceGitStatus).mockResolvedValue(statusWithRow());
    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply("const first = 1;"));
    await renderWorkspace();

    await plainClick("a-two");
    const diffId = await openActiveDiffTab();

    // The literal strings a snapshot pins: no host anywhere in a tab id.
    expect(chipIds()).toEqual([
      "workspace-session-tab-a-one",
      "workspace-session-tab-a-two",
      `workspace-session-tab-${diffId}`,
    ]);
    expect(diffId).toBe("tool:diff:workspace-1:src%2Fwriter.ts");
    expect(selectedChips()).toEqual([`workspace-session-tab-${diffId}`]);

    // The tool pane is the Diff tab, under its own name.
    const pane = document.querySelector<HTMLElement>("#workspace-panel-terminal");
    expect(pane?.getAttribute("aria-label")).toBe("Diff");
    expect(pane?.textContent).toContain("const first = 1;");

    // The side panel is the same element with the same name, and it remounts
    // with the workspace so no draft or menu survives the switch.
    const before = sidePanelBody();
    expect(before?.getAttribute("aria-label")).toBe("Changes");
    expect(before?.getAttribute("role")).toBe("tabpanel");
    await showWorkspace("beta");
    expect(sidePanelBody()).not.toBe(before);
    expect(sidePanelBody()?.getAttribute("aria-label")).toBe("Changes");

    // The sidebar row for the workspace on screen is the pressed one, and no
    // host badge is painted on any row.
    expect(sidebarRow("beta").getAttribute("aria-pressed")).toBe("true");
    expect(sidebarRow("alpha").getAttribute("aria-pressed")).toBe("false");
    expect(sidebarRow("alpha").textContent).not.toContain("local");
    expect(document.querySelector(".workspace-session-host-badge")).toBeNull();
  });
});
