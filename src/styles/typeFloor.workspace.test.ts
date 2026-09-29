// The workspace slice's 12px type floor (SPEC-tokens: "Nothing below 12",
// with the avatar exception the same spec records after that line). A
// parallel slice keeps its own sibling walk, so this file is self-contained
// on purpose.
// @vitest-environment node
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { findBelowTypeFloor } from "./typeFloor";

const rootDir = resolve(import.meta.dirname, "../..");
const SHEET_PATHS = [
  "src/styles/tokens.css",
  "src/features/workspace/Workspace.css",
  "src/features/workspace/QueueTrack.css",
  "src/features/workspace/paneHeader/paneHeader.css",
  "src/features/workspace/sidebar/sidebar.css",
  "src/features/workspace/strip/strip.css",
  "src/features/workspace/panel/changes.css",
  "src/features/workspace/panel/files.css",
  "src/features/workspace/AgentTaskPill.css",
  "src/features/workspace/timeline/timeline.css",
  "src/features/workspace/SubagentMenu.css",
  "src/features/workspace/fileTab.css",
  "src/features/workspace/panel/panel.css",
  "src/features/workspace/panel/diffTab.css",
];
const SHEETS = SHEET_PATHS.map((path) => readFileSync(resolve(rootDir, path), "utf8"));

// Text-as-icon glyphs keep the size their spec pins: SPEC-tokens, the line
// after "Nothing below 12", excepts the sidebar avatar initials at 11 on
// their 16/18px tiles (SPEC-regions §Sidebar). A rule earns the exemption
// only when its selector list is exactly an avatar selector — any other
// selector in the list makes it readable text and fails.
const GLYPH_SIZES: ReadonlyMap<string, number> = new Map([
  [".sidebar-avatar-project", 11],
  [".sidebar-avatar-workspace", 11],
]);

function exempt(finding: { rule: string; px: number | null }): boolean {
  const parts = finding.rule.split(",").map((part) => part.replace(/\s+/g, " ").trim());
  return parts.every((part) => GLYPH_SIZES.get(part) === finding.px);
}

describe("the workspace slice's 12px type floor", () => {
  it("walks every sheet the slice owns", () => {
    expect(SHEET_PATHS).toHaveLength(14);
  });

  it("declares no text size below 12px across the slice's sheets", () => {
    expect(findBelowTypeFloor(SHEETS).filter((finding) => !exempt(finding))).toEqual([]);
  });
});

describe("the avatar exemption", () => {
  it("keeps the two avatar tiles at their pinned 11", () => {
    const sheets = [
      ".sidebar-avatar-project { font-size: 11px; }",
      ".sidebar-avatar-workspace { font-size: 11px; }",
      ".sidebar-avatar-project, .sidebar-avatar-workspace { font-size: 11px; }",
    ];
    expect(findBelowTypeFloor(sheets).filter((finding) => !exempt(finding))).toEqual([]);
  });

  it("fails a selector that merely carries an avatar name", () => {
    const findings = findBelowTypeFloor([".sidebar-avatar-project-badge { font-size: 11px; }"]);
    expect(findings).toHaveLength(1);
    expect(findings[0]).toMatchObject({ rule: ".sidebar-avatar-project-badge", px: 11 });
  });

  it("fails a grouped rule that smuggles in a readable label", () => {
    const findings = findBelowTypeFloor([
      ".sidebar-avatar-project, .sidebar-row-title { font-size: 11px; }",
    ]);
    expect(findings).toHaveLength(1);
    expect(findings[0]).toMatchObject({
      rule: ".sidebar-avatar-project, .sidebar-row-title",
      px: 11,
    });
  });
});
