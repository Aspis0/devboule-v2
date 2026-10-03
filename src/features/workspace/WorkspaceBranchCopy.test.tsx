// @vitest-environment happy-dom
import { act, useRef } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Session, WorkspaceGitStatus } from "../../types/ipc";
import { workspaceGitStatus } from "../../lib/tauri";
import { rememberChangesStatus } from "./changesStatusCache";
import { PaneHeaderKebab } from "./paneHeader/PaneHeaderKebab";
import { headerMenu } from "./paneHeader/paneHeaderMenu";
import { SessionTabMenu } from "./strip/SessionTabMenu";
import { useTabCloseFlow } from "./strip/useTabCloseFlow";
import { buildTabCopyEntries, tabCopyValue } from "./strip/tabCopyActions";
import { composeStripTabs, makeToolTab } from "./strip/toolTabs";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

/** The workspace as the UI names it, for a fixture that only knows the daemon id. */
const keyFor = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  workspaceGitStatus: vi.fn(),
}));
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const session: Session = {
  id: "own-session",
  workspaceId: "own-workspace",
  kind: "terminal",
  title: "Own",
  state: { type: "live", generation: 1 },
  createdAtMs: 1,
  elapsedMs: 0,
};
const other: Session = { ...session, id: "active-session", workspaceId: "active-workspace" };
const status = (branch: string | null = "feature/session"): WorkspaceGitStatus => ({
  isGit: true,
  branch,
  dirty: false,
  totals: { additions: 0, deletions: 0 },
  rows: [],
  error: null,
});
let root: Root;
let host: HTMLDivElement;
const writeText = vi.fn(async (_text: string) => undefined);
function TabProbe() {
  const button = useRef<HTMLButtonElement>(null);
  const flow = useTabCloseFlow({
    sessions: [session, other],
    tabs: composeStripTabs([session, other], []),
    activeTabId: other.id,
    selection: new Set(),
    onClose: () => undefined,
    onCloseTools: () => undefined,
    onCloseTabs: () => undefined,
    selectTab: () => undefined,
    clearSelection: () => undefined,
    addButtonRef: button,
    renameMenu: { entriesFor: () => [], open: () => undefined },
  });
  return (
    <>
      <button
        id="workspace-session-tab-own-session"
        ref={button}
        onClick={() => flow.openMenu(session.id)}
      >
        Open
      </button>
      <SessionTabMenu
        open={flow.menu !== null}
        anchorRef={flow.anchorRef}
        entries={flow.menu?.entries ?? []}
        onEntry={flow.activateEntry}
        copyEntryValue={flow.copyEntryValue}
        onClose={flow.closeMenu}
      />
    </>
  );
}
function HeaderProbe() {
  const menu = headerMenu(
    undefined,
    {
      workspaceKey: keyFor(session.workspaceId ?? ""),
      closeEntries: [],
      onCloseEntry: () => undefined,
    },
    session.id,
  );
  return menu ? <PaneHeaderKebab menu={menu} /> : null;
}
const item = (label: string) =>
  [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(
    (entry) => entry.textContent === label,
  );
beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(workspaceGitStatus).mockReset().mockResolvedValue(status());
  rememberChangesStatus(keyFor("own-workspace"), null);
  rememberChangesStatus(keyFor("active-workspace"), status("wrong-active-branch"));
  writeText.mockReset().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  vi.useRealTimers();
  Reflect.deleteProperty(navigator, "clipboard");
});
async function open(kind: string) {
  await act(async () => root.render(kind === "tab" ? <TabProbe /> : <HeaderProbe />));
  await act(async () => host.querySelector("button")!.click());
}

describe.each(["tab", "header"])("%s branch copy", (kind) => {
  it("copies the session workspace branch with another workspace active", async () => {
    await open(kind);
    expect(workspaceGitStatus).toHaveBeenCalledExactlyOnceWith("own-workspace");
    await act(async () => item("Copy branch name")!.click());
    expect(writeText).toHaveBeenCalledExactlyOnceWith("feature/session");
    expect(document.querySelector('[role="status"]')?.textContent).toBe("Branch name copied");
    expect(item("Copied")).toBeDefined();
    await act(async () => vi.advanceTimersByTimeAsync(1500));
    expect(item("Copy branch name")).toBeDefined();
  });

  it("announces branch clipboard failure", async () => {
    writeText.mockRejectedValueOnce(new Error("denied"));
    await open(kind);
    await act(async () => item("Copy branch name")!.click());
    expect(item("Copy failed")).toBeDefined();
    expect(document.querySelector('[role="status"]')?.textContent).toBe("Branch name copy failed");
  });

  it.each([null, "", " ", "(detached)", "HEAD"])("omits branch item for %s", async (branch) => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(status(branch));
    await open(kind);
    expect(item("Copy branch name")).toBeUndefined();
  });

  it.each(["non-git", "error", "rejection"])("omits branch item for %s", async (edge) => {
    if (edge === "non-git")
      vi.mocked(workspaceGitStatus).mockResolvedValue({ ...status(), isGit: false });
    if (edge === "error")
      vi.mocked(workspaceGitStatus).mockResolvedValue({ ...status(), error: "failed" });
    if (edge === "rejection") vi.mocked(workspaceGitStatus).mockRejectedValue(new Error("failed"));
    await open(kind);
    expect(item("Copy branch name")).toBeUndefined();
  });

  it("omits the item while loading and ignores a reply after close", async () => {
    let complete!: (value: WorkspaceGitStatus) => void;
    vi.mocked(workspaceGitStatus).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }),
    );
    await open(kind);
    expect(item("Copy branch name")).toBeUndefined();
    await act(async () =>
      document
        .querySelector('[role="menu"]')!
        .dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })),
    );
    await act(async () => complete(status("late")));
    expect(document.querySelector('[role="menu"]')).toBeNull();
    await act(async () => host.querySelector("button")!.click());
    await act(async () => item("Copy branch name")!.click());
    expect(writeText).toHaveBeenCalledExactlyOnceWith("feature/session");
  });
});

it("never offers a workspace branch on tool tabs", () => {
  const [tab] = composeStripTabs([], [makeToolTab("file", keyFor("own-workspace"), "a.ts")]);
  expect(buildTabCopyEntries(tab, "feature/session").map((entry) => entry.key)).not.toContain(
    "copy-branch-name",
  );
  expect(tabCopyValue(tab, "copy-branch-name", "feature/session")).toBeNull();
});
