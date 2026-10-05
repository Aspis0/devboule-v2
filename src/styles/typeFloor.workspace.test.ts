// The workspace slice's 12px type floor (SPEC-tokens: "Nothing below 12").
// A parallel slice keeps its own sibling walk, so this file is self-contained
// on purpose.
// @vitest-environment node
import { readFileSync } from "node:fs";
import { basename, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { findBelowTypeFloor } from "./typeFloor";

const rootDir = resolve(import.meta.dirname, "../..");
const SHEET_PATHS = [
  "src/styles/tokens.css",
  "src/features/workspace/Workspace.css",
  "src/features/workspace/QueueTrack.css",
  "src/features/workspace/paneHeader/paneHeader.css",
  "src/features/workspace/paneHeader/GoalLine.css",
  "src/features/workspace/sidebar/sidebar.css",
  "src/features/workspace/strip/strip.css",
  "src/features/workspace/panel/changes.css",
  "src/features/workspace/panel/files.css",
  "src/features/workspace/AgentTaskPill.css",
  "src/features/workspace/timeline/timeline.css",
  "src/features/workspace/SubagentMenu.css",
  "src/features/workspace/fileTab.css",
  "src/features/workspace/OpenInEditorAction.css",
  "src/features/workspace/panel/panel.css",
  "src/features/workspace/panel/diffTab.css",
  "src/features/workspace/split/SplitPane.css",
];
const SHEETS = SHEET_PATHS.map((path) => readFileSync(resolve(rootDir, path), "utf8"));

// Nothing in the slice is excepted: every size these sheets declare — the
// sidebar's avatar initials included — paints at 12px or above.

describe("the workspace slice's 12px type floor", () => {
  it("walks exactly the sixteen workspace sheets plus tokens, by name", () => {
    expect(SHEET_PATHS.map((path) => basename(path))).toEqual([
      "tokens.css",
      "Workspace.css",
      "QueueTrack.css",
      "paneHeader.css",
      "GoalLine.css",
      "sidebar.css",
      "strip.css",
      "changes.css",
      "files.css",
      "AgentTaskPill.css",
      "timeline.css",
      "SubagentMenu.css",
      "fileTab.css",
      "OpenInEditorAction.css",
      "panel.css",
      "diffTab.css",
      "SplitPane.css",
    ]);
  });

  it("declares no text size below 12px across the slice's sheets, in either theme", () => {
    const findings = [
      ...findBelowTypeFloor(SHEETS, "light"),
      ...findBelowTypeFloor(SHEETS, "dark"),
    ];
    expect(findings).toEqual([]);
  });
});

describe("the sidebar avatar's size", () => {
  it("carries no exemption: an 11px avatar is a finding like any other text", () => {
    const findings = findBelowTypeFloor([".sidebar-avatar-project { font-size: 11px; }"]);
    expect(findings).toHaveLength(1);
    expect(findings[0]).toMatchObject({ rule: ".sidebar-avatar-project", px: 11 });
  });

  it("fails a grouped rule that smuggles in a readable label", () => {
    const findings = findBelowTypeFloor([
      ".sidebar-avatar-project, .workspace-row-title { font-size: 11px; }",
    ]);
    expect(findings).toHaveLength(1);
    expect(findings[0]).toMatchObject({
      rule: ".sidebar-avatar-project, .workspace-row-title",
      px: 11,
    });
  });
});
