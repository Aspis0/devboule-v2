import { describe, expect, it } from "vitest";
import type { Session } from "../../../types/ipc";
import { buildTabCopyEntries, isTabCopyAction, tabCopyValue } from "./tabCopyActions";
import { composeStripTabs, makeBrowserTab, makeToolTab } from "./toolTabs";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";

const keyFor = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

describe("single-tab copy actions", () => {
  it.each([
    ["acp", ["Copy path"]],
    ["terminal", ["Copy session ID", "Copy path"]],
  ] as const)("copies the stable ID and display cwd for %s", (kind, labels) => {
    const session = makeSession(kind);
    const [tab] = composeStripTabs([{ ...session, cwd: String.raw`\\?\C:\my project` }], []);
    expect(buildTabCopyEntries(tab).map((entry) => entry.label)).toEqual(labels);
    expect(tabCopyValue(tab, "copy-session-id")).toBe(session.id);
    expect(tabCopyValue(tab, "copy-path")).toBe(String.raw`C:\my project`);
    expect(buildTabCopyEntries(tab).at(-1)?.separatorAfter).toBe(true);
  });

  it.each([undefined, ""])("omits Copy path with cwd %s", (cwd) => {
    const [tab] = composeStripTabs([{ ...makeSession("acp"), cwd }], []);
    expect(buildTabCopyEntries(tab)).toEqual([]);
    expect(tabCopyValue(tab, "copy-path")).toBeNull();
  });

  it.each(["file", "diff"] as const)("offers only the file path on a %s tab", (kind) => {
    const [tab] = composeStripTabs([], [makeToolTab(kind, keyFor("ws"), "src/my file.ts")]);
    expect(buildTabCopyEntries(tab)).toEqual([
      { key: "copy-path", label: "Copy relative path", disabled: false, separatorAfter: true },
    ]);
    expect(tabCopyValue(tab, "copy-path")).toBe("src/my file.ts");
    expect(tabCopyValue(tab, "copy-session-id")).toBeNull();
  });

  it("adds Copy branch name only to session tabs with an eligible branch", () => {
    const [tab] = composeStripTabs([makeSession("acp")], []);
    expect(buildTabCopyEntries(tab, "feature/work").map((entry) => entry.key)).toEqual([
      "copy-branch-name",
    ]);
    expect(buildTabCopyEntries(tab, "feature/work").at(-1)?.separatorAfter).toBe(true);
    expect(tabCopyValue(tab, "copy-branch-name", "feature/work")).toBe("feature/work");
    expect(tabCopyValue(tab, "copy-branch-name", "(detached)")).toBeNull();
    expect(isTabCopyAction("copy-branch-name")).toBe(true);
    expect(isTabCopyAction("close")).toBe(false);
  });

  it("preserves a bare verbatim prefix instead of producing an empty copy", () => {
    const [tab] = composeStripTabs([{ ...makeSession("acp"), cwd: "\\\\?\\" }], []);
    expect(tabCopyValue(tab, "copy-path")).toBe("\\\\?\\");
    expect(buildTabCopyEntries(tab).map((entry) => entry.label)).toContain("Copy path");
  });

  it("offers a browser tab its address, never its id as a path", () => {
    const [tab] = composeStripTabs([], [makeBrowserTab(keyFor("ws"), "browser-1")]);
    const address = (): string | null => "https://example.com/docs";

    expect(buildTabCopyEntries(tab, null, address)).toEqual([
      { key: "copy-address", label: "Copy address", disabled: false, separatorAfter: true },
    ]);
    expect(tabCopyValue(tab, "copy-address", null, address)).toBe("https://example.com/docs");
    // The tab's id is an opaque handle, never something to put on a clipboard.
    expect(tabCopyValue(tab, "copy-path", null, address)).toBeNull();
    expect(isTabCopyAction("copy-address")).toBe(true);
  });

  it("offers a browser tab nothing at all when its address is unknown", () => {
    const [tab] = composeStripTabs([], [makeBrowserTab(keyFor("ws"), "browser-1")]);

    expect(buildTabCopyEntries(tab, null, () => null)).toEqual([]);
    expect(tabCopyValue(tab, "copy-address", null, () => null)).toBeNull();
  });
});

function makeSession(kind: Session["kind"]): Session {
  return {
    id: "stable-id",
    workspaceId: "ws",
    kind,
    title: "Human title",
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
  };
}
