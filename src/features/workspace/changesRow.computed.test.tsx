// @vitest-environment happy-dom
// The Changes file row's no-overlap decisions, through the real sheets in
// bundle order: the name is the yielding text, status/counts/acts stay out
// of each other's boxes, the acts wrap right-aligned on coarse pointers and
// reveal as a trailing overlay on fine ones, and the select button's floor
// covers its own fixed chrome at every depth. The proof helper descends
// into @media and drops its context, so the pointer gate itself is read
// from the source, brace-aware.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceGitRow } from "../../types/ipc";
import { parseRules, stripComments } from "../../styles/cssText";
import { ChangesTreeView } from "./ChangesTreeView";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import {
  assembleCssProof,
  removeCssProof,
  selectorMatches,
  specificity,
  type CssRule,
} from "./cssProof";

const keyFor = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../../..");
const CHANGES_CSS = "src/features/workspace/panel/changes.css";

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

// Bundle order per the import graph, as ChangesSurfaceTree.test assembles it.
const sheets = [
  read("src/styles/tokens.css"),
  read("src/styles/global.css"),
  read("src/features/workspace/Workspace.css"),
  read(CHANGES_CSS),
  read("src/features/workspace/strip/strip.css"),
  read("src/features/workspace/panel/panel.css"),
];

const FINE_POINTER_MEDIA = "@media (hover: hover) and (pointer: fine)";

/** changes.css split at the fine-pointer media block: the rules outside it
 *  and the rules inside, each in source order. The proof helper flattens
 *  at-rules without their context, so only the source says which
 *  declaration the pointer gate carries. */
function splitByMediaGate(css: string): { base: CssRule[]; gated: CssRule[] } {
  const source = stripComments(css);
  const gated: CssRule[] = [];
  let base = "";
  let cursor = 0;
  for (;;) {
    const at = source.indexOf(FINE_POINTER_MEDIA, cursor);
    if (at < 0) {
      base += source.slice(cursor);
      break;
    }
    const open = source.indexOf("{", at);
    if (open < 0) throw new Error("media block without a body");
    let depth = 1;
    let close = open + 1;
    while (close < source.length && depth > 0) {
      if (source[close] === "{") depth += 1;
      else if (source[close] === "}") depth -= 1;
      close += 1;
    }
    if (depth !== 0) throw new Error("unterminated media block");
    gated.push(...parseRules(source.slice(open + 1, close - 1)));
    base += source.slice(cursor, at);
    cursor = close;
  }
  return { base: parseRules(base), gated };
}

function ruleFor(rules: readonly CssRule[], target: string): CssRule {
  const found = rules.find((rule) => selectorMatches(rule.selector, target));
  if (found === undefined) throw new Error(`no rule declares ${target}`);
  return found;
}

function row(overrides: Partial<WorkspaceGitRow> & { path: string }): WorkspaceGitRow {
  return { additions: 0, deletions: 0, status: "modified", capped: false, ...overrides };
}

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  removeCssProof();
  vi.clearAllMocks();
});

/** Render the tree over three rows — new, plain, deleted (the last one
 * nested, so its floor carries the indent) — and inject every rule that
 * names the row family: the whole cascade, not a hand-picked subset.
 * Returns the assembled sheets' token lookup for the colour pins. */
async function renderRowFamily(): Promise<{ token: (name: string) => string | undefined }> {
  await act(async () => {
    root.render(
      <ChangesTreeView
        rows={[
          row({ path: "README.md", status: "untracked", additions: 5, deletions: 2 }),
          row({ path: "CHANGELOG.md", additions: 1, deletions: 1 }),
          row({ path: "src/sub/b.ts", status: "deleted", deletions: 9 }),
        ]}
        inexact={false}
        selection={null}
        onSelect={vi.fn()}
        onStage={vi.fn()}
        onUnstage={vi.fn()}
        onDiscard={vi.fn()}
        menuPath={null}
        onToggleMenu={vi.fn()}
        onCloseMenu={vi.fn()}
        acting={false}
        workspaceKey={keyFor("ws-1")}
      />,
    );
  });
  const css = assembleCssProof(sheets);
  const targets = css.rules
    .flatMap((rule) => rule.selector.split(","))
    .map((part) => part.trim())
    .filter(
      (part) =>
        part === "*" ||
        part.includes(".workspace-file-change") ||
        part.includes(".workspace-changes-file"),
    );
  css.inject(targets);
  return { token: css.token };
}

function rowAt(index: number): HTMLElement {
  const rows = container.querySelectorAll<HTMLElement>(".workspace-file-change-row");
  const found = rows[index];
  if (found === undefined) throw new Error(`file row ${index} did not render`);
  return found;
}

describe("the changes file row's computed layout decisions", () => {
  it("keeps the row's parts as siblings, wraps right-aligned, and names the select for AT", async () => {
    const { token } = await renderRowFamily();

    const topRow = rowAt(0);
    const rowStyle = getComputedStyle(topRow);
    expect(rowStyle.justifyContent).toBe("flex-end");

    const button = topRow.querySelector(".workspace-file-change");
    const status = topRow.querySelector(".workspace-file-change-status");
    const stats = topRow.querySelector(".workspace-file-change-stats");
    const actions = topRow.querySelector(".workspace-file-change-actions");
    if (button === null || status === null || stats === null || actions === null) {
      throw new Error("the row's four parts did not render");
    }
    // The fixed halves live on the row itself, so the wrap counts them at
    // full width; inside the button a starved line could never see them.
    expect(status.parentElement).toBe(topRow);
    expect(stats.parentElement).toBe(topRow);
    expect(button.parentElement).toBe(topRow);

    // The select's name carries what its text no longer shows: status and
    // counts, as one string, while the visible row stays uncluttered.
    expect(button.getAttribute("aria-label")).toBe("README.md untracked +5 −2");
    expect(button.textContent).toBe("README.md");
    expect(getComputedStyle(actions).flexShrink).toBe("0");
    expect(getComputedStyle(stats).whiteSpace).toBe("nowrap");

    // Tones travel with the row's classes: new files in the add tone, a
    // plain row in the quiet counts colour, deletions in the danger tone.
    expect(getComputedStyle(stats).color).toBe(token("--tone-add"));
    const plainStats = rowAt(1).querySelector(".workspace-file-change-stats");
    const deletedStats = rowAt(2).querySelector(".workspace-file-change-stats");
    if (plainStats === null || deletedStats === null) {
      throw new Error("a row's counts did not render");
    }
    expect(getComputedStyle(plainStats).color).toBe(token("--muted"));
    expect(getComputedStyle(deletedStats).color).toBe(token("--danger"));
  });

  it("makes the name the yielding text: zero minimum, clip, ellipsis", async () => {
    await renderRowFamily();

    const name = rowAt(0).querySelector(".workspace-file-change-name");
    if (name === null) throw new Error("file name did not render");
    const nameStyle = getComputedStyle(name);
    expect(Number.parseFloat(nameStyle.minWidth)).toBe(0);
    expect(nameStyle.overflow).toBe("hidden");
    expect(nameStyle.textOverflow).toBe("ellipsis");
    expect(nameStyle.whiteSpace).toBe("nowrap");
  });

  it("floors the select button above its own fixed chrome, at every depth", async () => {
    await renderRowFamily();

    for (const index of [0, 2]) {
      const button = rowAt(index).querySelector(".workspace-file-change");
      if (button === null) throw new Error("row button did not render");
      const icon = button.querySelector(".workspace-changes-file-icon");
      if (icon === null) throw new Error("row icon did not render");
      const style = getComputedStyle(button);
      const fixedChrome =
        Number.parseFloat(style.paddingLeft) +
        Number.parseFloat(style.paddingRight) +
        Number.parseFloat(style.gap) +
        Number.parseFloat(getComputedStyle(icon).width);
      // The floor must cover pad + icon + gap, or the line-break lets the
      // button starve below its own padding and the name spills over the
      // status word; the remaining 16px is the name's room above the floor.
      expect(Number.parseFloat(style.minWidth)).toBeGreaterThanOrEqual(fixedChrome + 16);
    }
    // The overlay's containing block is the row.
    expect(getComputedStyle(rowAt(0)).position).toBe("relative");
  });

  it("holds the coarse base: wrapped right-aligned acts, fixed halves, menu below the row", () => {
    const { base } = splitByMediaGate(read(CHANGES_CSS));

    const rowRule = ruleFor(base, ".workspace-file-change-row");
    expect(rowRule.body).toContain("flex-wrap: wrap");
    expect(rowRule.body).toContain("justify-content: flex-end");
    expect(ruleFor(base, ".workspace-file-change-status").body).toContain("flex: none");
    expect(ruleFor(base, ".workspace-file-change-stats").body).toContain("flex: none");
    expect(ruleFor(base, ".workspace-file-change-actions").body).toContain("flex: none");
    // The row is the overlay's containing block: nothing between it and
    // the acts claims a position of its own.
    expect(ruleFor(base, ".workspace-file-change").body).not.toContain("position");
    expect(ruleFor(base, ".workspace-file-change-row .workspace-tree-menu").body).toContain(
      "top: 100%",
    );
  });

  it("gates the one-line overlay behind the fine-pointer query", () => {
    const { gated } = splitByMediaGate(read(CHANGES_CSS));
    expect(gated.length).toBeGreaterThan(0);

    // The line itself is pinned inside the gate: the flex breaker would
    // wrap before the yield rule below can shrink anything.
    expect(ruleFor(gated, ".workspace-file-change-row").body).toContain("flex-wrap: nowrap");

    const hidden = ruleFor(gated, ".workspace-file-change-row .workspace-file-change-actions");
    expect(hidden.body).toContain("position: absolute");
    expect(hidden.body).toContain("right: 0");
    expect(hidden.body).toContain("opacity: 0");
    expect(hidden.body).toContain("visibility: hidden");
    expect(hidden.body).toContain("pointer-events: none");

    const besidePencil = ruleFor(
      gated,
      ".workspace-file-change-row:has(.workspace-changes-pencil) .workspace-file-change-actions",
    );
    expect(besidePencil.body).toContain("right: 26px");

    const reveal = ruleFor(
      gated,
      ".workspace-file-change-row:hover .workspace-file-change-actions",
    );
    expect(reveal.selector).toContain(
      ".workspace-file-change-row:focus-within .workspace-file-change-actions",
    );
    expect(reveal.selector).toContain(
      ".workspace-file-change-row:has(.workspace-tree-menu) .workspace-file-change-actions",
    );
    expect(reveal.body).toContain("opacity: 1");
    expect(reveal.body).toContain("visibility: visible");
    expect(reveal.body).toContain("pointer-events: auto");

    // Status and counts step aside while the acts show — same three states.
    const aside = ruleFor(gated, ".workspace-file-change-row:hover .workspace-file-change-status");
    expect(aside.selector).toContain(
      ".workspace-file-change-row:hover .workspace-file-change-stats",
    );
    expect(aside.selector).toContain(
      ".workspace-file-change-row:has(.workspace-tree-menu) .workspace-file-change-stats",
    );
    expect(aside.body).toContain("visibility: hidden");

    // The last-resort yield: clipped and ellipsized, never spilling.
    const yieldRule = ruleFor(gated, ".workspace-file-change-row .workspace-file-change-status");
    expect(yieldRule.selector).toContain(".workspace-file-change-row .workspace-file-change-stats");
    expect(yieldRule.body).toContain("flex: 0 1 auto");
    expect(yieldRule.body).toContain("min-width: 0");
    expect(yieldRule.body).toContain("overflow: hidden");
    expect(yieldRule.body).toContain("text-overflow: ellipsis");
  });

  it("masks the name's tail behind the revealed strip, in the row's own tokens", () => {
    const { base, gated } = splitByMediaGate(read(CHANGES_CSS));

    // One mask per revealed state, all inside the gate: ground for
    // focus/menu, the hover tint, the selected fill — no raw hex.
    const masks = gated.filter((rule) => rule.body.includes("background-image"));
    const described = masks
      .map((rule) => `${rule.selector} { ${rule.body} }`)
      .join("\n")
      .replace(/\s+/g, " ");
    expect(masks.length).toBeGreaterThanOrEqual(3);
    expect(described).toContain("var(--panel-side)");
    expect(described).toContain("var(--fill-tool)");
    expect(described).toContain("transparent");
    expect(described).toContain("16px");
    expect(described).not.toMatch(/#[0-9a-f]{3,8}/i);
    // The hover tint is the row's own fill at its own share: 7% ink.
    expect(described).toContain("color-mix(in srgb, var(--ink) 7%, var(--panel-side))");
    expect(described).toContain(".workspace-file-change-row:hover .workspace-file-change-actions");
    expect(described).toContain(
      ".workspace-file-change-row:has(.workspace-file-change-selected) .workspace-file-change-actions",
    );
    expect(described).toContain(
      ".workspace-file-change-row:focus-within .workspace-file-change-actions",
    );

    // All three share one specificity, so sheet order is the cascade:
    // ground, then hover tint, then the selected fill winning last.
    expect(masks.map((rule) => specificity(rule.selector))).toEqual([
      [0, 3, 0],
      [0, 3, 0],
      [0, 3, 0],
    ]);
    expect(masks[0]!.selector).toContain(":focus-within");
    expect(masks[1]!.selector).toContain(":hover");
    expect(masks[2]!.selector).toContain(".workspace-file-change-selected");

    // The coarse side carries none of it: the acts stay in the wrapped
    // flow, unmasked — a lifted or masked base rule would break touch.
    for (const declaration of ["background-image", "position: absolute", "visibility: hidden"]) {
      const leaked = base.filter(
        (rule) =>
          rule.selector.includes(".workspace-file-change-actions") &&
          rule.body.includes(declaration),
      );
      expect(leaked, declaration).toHaveLength(0);
    }
  });

  it("keeps the mask's tokens defined in both themes", () => {
    for (const theme of ["light", "dark"] as const) {
      const css = assembleCssProof(sheets, theme);
      for (const name of ["--panel-side", "--fill-tool", "--ink"]) {
        const value = css.token(name);
        expect(value, `${name} in ${theme}`).toBeDefined();
        expect(value, `${name} in ${theme}`).not.toBe("");
      }
    }
  });
});
