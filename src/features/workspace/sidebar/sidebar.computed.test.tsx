// @vitest-environment happy-dom

// Styling proof for the sidebar slice without launching the app. The REAL
// stylesheets are assembled in cssProof.ts (comments stripped, brace-matched,
// @-blocks skipped), the rules the sidebar depends on are extracted from
// them, their tokens are resolved to the light values, and those exact rules
// are injected into the document so the computed styles of the rendered
// chrome can be asserted. A rule swallowed by a malformed comment (the live
// defect) or a selector that never matches the markup fails these
// assertions. The strip's own proof lives in strip/strip.computed.test.tsx.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { beforeEachHarness, renderWorkspace, unmountWorkspace } from "../bulkCloseHarness";
import { assembleCssProof, removeCssProof } from "../cssProof";

const rootDir = resolve(import.meta.dirname, "../../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await unmountWorkspace();
  removeCssProof();
});

describe("sidebar computed styles (real stylesheets, no app launch)", () => {
  // Sheet order matches the bundle's import graph: tokens, global, then
  // Workspace.css — pulled in by NewProjectDialog, which Workspace imports
  // at :15, before the registry chain (:16) that pulls in changes.css —
  // then changes.css, strip (SessionStrip, :23), sidebar (Sidebar, :37).
  // First importer wins the position, so the :80 re-import of
  // Workspace.css changes nothing.
  const { rulesFor, inject, token } = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/workspace/Workspace.css"),
    read("src/features/workspace/panel/changes.css"),
    read("src/features/workspace/strip/strip.css"),
    read("src/features/workspace/sidebar/sidebar.css"),
  ]);

  it("workspace rows are laid out as spec'd: flex, padded 5/12/6 as Orca's cards, left-aligned", async () => {
    const body = rulesFor(".workspace-row");
    expect(body).toContain("padding: 5px 12px 6px");

    inject([".workspace-row"]);
    await renderWorkspace();
    const row = document.querySelector<HTMLElement>(".workspace-row");
    if (row === null) throw new Error("workspace row did not render");
    const style = getComputedStyle(row);
    expect(style.display).toBe("flex");
    expect(style.textAlign).toBe("left");
    expect(style.paddingLeft).toBe("12px");
    expect(style.paddingTop).toBe("5px");
    expect(style.paddingBottom).toBe("6px");
  });

  it("spaces the list itself by 4px", async () => {
    expect(rulesFor(".workspace-project-items")).toContain("gap: 4px");
    inject([".workspace-project-items"]);
    await renderWorkspace();
    const items = document.querySelector<HTMLElement>(".workspace-project-items");
    if (items === null) throw new Error("the project's item list did not render");
    expect(getComputedStyle(items).gap).toBe("4px");
  });

  it("the row button fills its wrap, so the age sits against the right edge", async () => {
    // A button shrink-wraps its content by default: the row measured 113px
    // inside a 195px wrap, so the age had nothing to right-align against.
    expect(rulesFor(".workspace-row")).toContain("width: 100%");
    inject([".workspace-row"]);
    await renderWorkspace();
    const row = document.querySelector<HTMLElement>(".workspace-row");
    if (row === null) throw new Error("workspace row did not render");
    expect(getComputedStyle(row).width).toBe("100%");
  });

  it("prints the name at 14px and the meta line at 12px, with no mono face on the row", () => {
    expect(rulesFor(".workspace-row-title")).toContain("font-size: 14px");
    expect(rulesFor(".workspace-row-title")).toContain("font-weight: 500");
    expect(rulesFor(".workspace-row-sub")).toContain("font-size: 12px");
    expect(rulesFor(".workspace-row-sub")).toContain(`color: ${token("--muted")}`);
    expect(rulesFor(".workspace-row-totals")).toContain("font-variant-numeric: tabular-nums");
    // The rail's rows carry no code face of their own: the branch alone is mono.
    expect(rulesFor(".workspace-row-title")).not.toContain("JetBrains Mono");
    expect(rulesFor(".workspace-row")).not.toContain("JetBrains Mono");
  });

  it("the selected row is the soft fill, and hover only tints", async () => {
    // A 40px row in the strong selected fill reads as a block; the soft one
    // keeps the row a row. happy-dom cannot mix colours, so the pin is on the
    // declaration and its token.
    const soft = (token("--fill-selected-soft") ?? "").replace(
      /var\(--[a-zA-Z0-9-]+\)/,
      token("--ground-center") ?? "",
    );
    expect(rulesFor(".workspace-row-selected")).toContain(`background: ${soft}`);
    inject([".workspace-row", ".workspace-row-selected"]);
    await renderWorkspace();
    const selected = document.querySelector<HTMLElement>(".workspace-row-selected");
    if (selected === null) throw new Error("no selected workspace row rendered");
    expect(getComputedStyle(selected).borderRadius).toBe("6px");
  });

  it("the selected row's meta line brightens to the primary ink, the only tone that clears its fill", () => {
    // The fill is warm and light; muted misses the 6.97 floor on it in both
    // themes, so the meta line paints with the name's own tone there.
    expect(rulesFor(".workspace-row-selected .workspace-row-sub")).toContain(
      `color: ${token("--ink")}`,
    );
  });

  it("the status slot is a 20px lane in the row's first place, and the project badge stays 16px", async () => {
    inject([".workspace-row-status", ".sidebar-avatar-project", ".sidebar-avatar"]);
    await renderWorkspace();
    const dot = document.querySelector<HTMLElement>(".workspace-row-status");
    if (dot === null) throw new Error("the status dot did not render");
    expect(getComputedStyle(dot).width).toBe("20px");
    expect(getComputedStyle(dot).height).toBe("20px");
    expect(getComputedStyle(dot).borderRadius).toBe("999px");
    expect(dot.parentElement?.firstElementChild).toBe(dot);

    const projectAvatar = document.querySelector<HTMLElement>(".sidebar-avatar-project");
    if (projectAvatar === null) throw new Error("project avatar did not render");
    expect(getComputedStyle(projectAvatar).width).toBe("16px");
    expect(getComputedStyle(projectAvatar).borderRadius).toBe("6px");
  });

  it("project header carries its spec padding", async () => {
    inject([".sidebar-project-head"]);
    await renderWorkspace();

    const projectHead = document.querySelector<HTMLElement>(".workspace-project-heading");
    if (projectHead === null) throw new Error("project header did not render");
    expect(getComputedStyle(projectHead).paddingLeft).toBe("12px");
  });

  it("the wordmark row keeps the wordmark whole", async () => {
    inject([".sidebar-top", ".sidebar-wordmark"]);
    await renderWorkspace();

    const top = document.querySelector<HTMLElement>(".sidebar-top");
    if (top === null) throw new Error("sidebar top did not render");
    expect(getComputedStyle(top).gap).toBe("0");

    const wordmarkRule = rulesFor(".sidebar-wordmark");
    expect(wordmarkRule).toContain("flex: none");
    expect(wordmarkRule).not.toContain("text-overflow");
    expect(wordmarkRule).not.toContain("overflow");
    const wordmark = document.querySelector<HTMLElement>(".sidebar-wordmark");
    if (wordmark === null) throw new Error("wordmark did not render");
    expect(getComputedStyle(wordmark).whiteSpace).toBe("nowrap");
  });

  it("the search takes its own row inside the top actions, the sidebar's whole width", async () => {
    // The actions carry the rail's own 12px a side, and the search row fills
    // them: no floor, no cap, the quiet row's own height. The field that
    // replaces the trigger fills the row in turn.
    expect(rulesFor(".sidebar-actions")).toContain("padding: 0 12px 8px");
    expect(rulesFor(".sidebar-search-row")).toContain("flex: none");
    expect(rulesFor(".sidebar-search-row")).toContain("height: 28px");
    // The field's own row is the whole width: no floor of its own, no cap to
    // shrink it at wide sidebars.
    expect(rulesFor(".sidebar-search")).not.toContain("max-width");
    inject([
      ".sidebar-search-row",
      ".sidebar-search",
      ".sidebar-search input",
      ".sidebar-search-trigger",
    ]);
    await renderWorkspace();

    const row = document.querySelector<HTMLElement>(".sidebar-search-row");
    if (row === null) throw new Error("the search row did not render");
    expect(getComputedStyle(row).height).toBe("28px");

    const trigger = document.querySelector<HTMLButtonElement>(".sidebar-search-trigger");
    if (trigger === null) throw new Error("the search trigger did not render");
    expect(getComputedStyle(trigger).width).toBe("100%");
    await act(async () => trigger.click());

    const pill = document.querySelector<HTMLElement>(".sidebar-search-row .sidebar-search");
    if (pill === null) throw new Error("the search field did not open");
    const style = getComputedStyle(pill);
    expect(style.height).toBe("100%");
    expect(style.flexGrow).toBe("1");

    const input = pill.querySelector("input");
    if (input === null) throw new Error("search input did not render");
    expect(getComputedStyle(input).paddingLeft).toBe("0px");
    expect(getComputedStyle(input).paddingRight).toBe("0px");
  });

  it("the resize handle keeps its 6px track and col-resize cursor", async () => {
    inject([".workspace-resize-handle"]);
    await renderWorkspace();
    const handle = document.querySelector<HTMLElement>(".workspace-resize-handle");
    if (handle === null) throw new Error("resize handle did not render");
    const style = getComputedStyle(handle);
    expect(style.width).toBe("6px");
    expect(style.cursor).toBe("col-resize");
  });

  it("diff removed lines keep their colour rule", () => {
    const body = rulesFor(".workspace-diff-removed");
    expect(body).toContain("color:");
  });

  it("the workspace mono group names only the code surfaces", () => {
    // The diff header is a UI bar (path + counts), so the shared mono group
    // must not claim it: a silent font swap here would print a header in a
    // different face from every other header.
    const mono = rulesFor(".workspace-diff-lines");
    expect(mono).toContain("JetBrains Mono");
    expect(rulesFor(".workspace-diff-header")).not.toContain("JetBrains Mono");
  });

  it("the R2a cleanup's deleted hover and focus rules are restored", () => {
    // The keyboard ring's neighbours: icon buttons and primary actions must
    // keep their :focus-visible rules (they were deleted by the R2a cleanup
    // and the merge would have dropped the ring from Send, New Project and
    // the close confirmation).
    for (const target of [
      ".workspace-icon-button:hover",
      ".workspace-icon-button:focus-visible",
      ".workspace-primary-action:hover",
      ".workspace-primary-action:focus-visible",
    ]) {
      const body = rulesFor(target);
      expect(body, `a rule for ${target} is missing`).not.toBe("");
      expect(body).toContain("background:");
    }
  });
});
