import { describe, expect, it } from "vitest";
import type { Session } from "../../../types/ipc";
import { buildTabCopyEntries, tabCopyValue } from "./tabCopyActions";
import { composeStripTabs, makeToolTab } from "./toolTabs";

describe("single-tab copy actions", () => {
  it.each(["acp", "terminal"] as const)("copies the stable ID and display cwd for %s", (kind) => {
    const session = makeSession(kind);
    const [tab] = composeStripTabs([{ ...session, cwd: String.raw`\\?\C:\my project` }], []);
    expect(buildTabCopyEntries(tab).map((entry) => entry.label)).toEqual([
      "Copy session ID",
      "Copy path",
    ]);
    expect(tabCopyValue(tab, "copy-session-id")).toBe(session.id);
    expect(tabCopyValue(tab, "copy-path")).toBe(String.raw`C:\my project`);
    expect(buildTabCopyEntries(tab).at(-1)?.separatorAfter).toBe(true);
  });

  it.each([undefined, ""])("omits Copy path with cwd %s", (cwd) => {
    const [tab] = composeStripTabs([{ ...makeSession("acp"), cwd }], []);
    expect(buildTabCopyEntries(tab).map((entry) => entry.key)).toEqual(["copy-session-id"]);
    expect(tabCopyValue(tab, "copy-path")).toBeNull();
  });

  it.each(["file", "diff"] as const)("offers only the file path on a %s tab", (kind) => {
    const [tab] = composeStripTabs([], [makeToolTab(kind, "ws", "src/my file.ts")]);
    expect(buildTabCopyEntries(tab)).toEqual([
      { key: "copy-path", label: "Copy relative path", disabled: false, separatorAfter: true },
    ]);
    expect(tabCopyValue(tab, "copy-path")).toBe("src/my file.ts");
    expect(tabCopyValue(tab, "copy-session-id")).toBeNull();
  });

  it("preserves a bare verbatim prefix instead of producing an empty copy", () => {
    const [tab] = composeStripTabs([{ ...makeSession("acp"), cwd: "\\\\?\\" }], []);
    expect(tabCopyValue(tab, "copy-path")).toBe("\\\\?\\");
    expect(buildTabCopyEntries(tab).map((entry) => entry.label)).toContain("Copy path");
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
