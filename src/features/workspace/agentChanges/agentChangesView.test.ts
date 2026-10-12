import { describe, expect, it } from "vitest";
import type { WorkspaceAgentEditFile } from "../../../types/ipc";
import { countParts } from "../changeCountParts";
import { editedAgo, editsText, filesText, sortedFiles, splitPath } from "./agentChangesView";

const file = (
  path: string,
  over: Partial<WorkspaceAgentEditFile> = {},
): WorkspaceAgentEditFile => ({
  path,
  additions: 0,
  deletions: 0,
  lastAgent: null,
  lastAtMs: 0,
  calls: [],
  truncated: false,
  ...over,
});

describe("the agent changes list", () => {
  it("sorts the files by path, so the list holds still while an agent keeps editing", () => {
    const sorted = sortedFiles([
      file("kalsa-site/index.html"),
      file("kalsa-site/assets/site.css"),
      file("mockups/a-route-selector/index.html"),
    ]).map((entry) => entry.path);
    expect(sorted).toEqual([
      "kalsa-site/assets/site.css",
      "kalsa-site/index.html",
      "mockups/a-route-selector/index.html",
    ]);
  });

  it("splits a path into its name and its folder", () => {
    expect(splitPath("kalsa-site/assets/site.css")).toEqual({
      name: "site.css",
      dir: "kalsa-site/assets",
    });
    expect(splitPath("README.md")).toEqual({ name: "README.md", dir: "" });
  });

  it("counts the files in the head, singular for one", () => {
    expect(filesText(1)).toBe("1 file");
    expect(filesText(4)).toBe("4 files");
  });

  it("reads a truncated count with a plus", () => {
    expect(filesText(4, true)).toBe("4+ files");
    expect(filesText(1, true)).toBe("1+ file");
  });

  it("counts a file's edits with a plus when its call list was cut", () => {
    expect(editsText(3, true)).toBe("3+ edits");
    expect(editsText(1, true)).toBe("1+ edit");
    expect(editsText(2, false)).toBe("2 edits");
  });
});

describe("the counts", () => {
  it("omits a zero side: a new file shows only its additions, a pure delete only its removals", () => {
    expect(countParts(212, 0)).toEqual({ add: "+212", del: null });
    expect(countParts(0, 9)).toEqual({ add: null, del: "−9" });
    expect(countParts(12, 30)).toEqual({ add: "+12", del: "−30" });
  });

  it("shows +0 when neither side moved, and compacts numbers above a thousand", () => {
    expect(countParts(0, 0)).toEqual({ add: "+0", del: null });
    expect(countParts(14_000, 1_100)).toEqual({ add: "+14k", del: "−1.1k" });
  });
});

describe("the age of an edit", () => {
  const NOW = 10_000_000;
  it("reads the age in minutes, then hours, then days", () => {
    expect(editedAgo(NOW - 30_000, NOW)).toBe("just now");
    expect(editedAgo(NOW - 10 * 60_000, NOW)).toBe("10 min ago");
    expect(editedAgo(NOW - 3 * 3_600_000, NOW)).toBe("3 h ago");
    expect(editedAgo(NOW - 2 * 86_400_000, NOW)).toBe("2 d ago");
  });
});
