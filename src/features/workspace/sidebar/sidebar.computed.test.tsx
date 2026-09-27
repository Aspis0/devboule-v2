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
  // Sheet order matches the bundle: tokens, global, strip (pulled in by
  // SessionStrip, which Workspace imports before its own CSS), workspace,
  // sidebar.
  const { rulesFor, inject } = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/workspace/strip/strip.css"),
    read("src/features/workspace/Workspace.css"),
    read("src/features/workspace/sidebar/sidebar.css"),
  ]);

  it("workspace rows are laid out as spec'd: flex, padded 4/8, left-aligned", async () => {
    const body = rulesFor(".workspace-row");
    expect(body).toContain("padding: 4px 8px");
    expect(body).toContain("min-height: 36px");

    inject([".workspace-row"]);
    await renderWorkspace();
    const row = document.querySelector<HTMLElement>(".workspace-row");
    if (row === null) throw new Error("workspace row did not render");
    const style = getComputedStyle(row);
    expect(style.display).toBe("flex");
    expect(style.textAlign).toBe("left");
    expect(style.paddingLeft).toBe("8px");
    expect(style.paddingTop).toBe("4px");
    expect(style.minHeight).toBe("36px");
  });

  it("avatars hold exactly one letter at 18px, and the project avatar 16px", async () => {
    inject([".sidebar-avatar-workspace", ".sidebar-avatar-project", ".sidebar-avatar"]);
    await renderWorkspace();
    const avatar = document.querySelector<HTMLElement>(".sidebar-avatar-workspace");
    if (avatar === null) throw new Error("workspace avatar did not render");
    // One grapheme, never the truncated first cut.
    expect(avatar.textContent).toHaveLength(1);
    expect(avatar.textContent).not.toContain("…");
    expect(getComputedStyle(avatar).width).toBe("18px");
    expect(getComputedStyle(avatar).borderRadius).toBe("5px");

    const projectAvatar = document.querySelector<HTMLElement>(".sidebar-avatar-project");
    if (projectAvatar === null) throw new Error("project avatar did not render");
    expect(getComputedStyle(projectAvatar).width).toBe("16px");
  });

  it("host header, project header, new row and foot carry their spec padding", async () => {
    inject([".sidebar-host-head", ".sidebar-project-head", ".workspace-new-row", ".sidebar-foot"]);
    await renderWorkspace();
    const hostHead = document.querySelector<HTMLElement>(".sidebar-host-head");
    if (hostHead === null) throw new Error("host header did not render");
    expect(hostHead.textContent).toContain("This PC");
    expect(getComputedStyle(hostHead).paddingLeft).toBe("8px");
    expect(getComputedStyle(hostHead).height).toBe("28px");

    const projectHead = document.querySelector<HTMLElement>(".workspace-project-heading");
    if (projectHead === null) throw new Error("project header did not render");
    expect(getComputedStyle(projectHead).paddingLeft).toBe("8px");

    const newRow = document.querySelector<HTMLElement>(".workspace-new-row");
    if (newRow === null) throw new Error("new workspace row did not render");
    expect(getComputedStyle(newRow).height).toBe("28px");
    expect(getComputedStyle(newRow).paddingLeft).toBe("8px");

    const foot = document.querySelector<HTMLElement>(".sidebar-foot");
    if (foot === null) throw new Error("daemon foot did not render");
    expect(foot.textContent).toContain("Daemon");
    expect(getComputedStyle(foot).paddingLeft).toBe("8px");
  });

  it("the search pill keeps the placeholder readable and the header fits at the 200px floor", async () => {
    // Live check found the input at 39px inside a 44.5px pill ("Searc").
    // happy-dom computes no layout, so this pins the BUDGET instead: at the
    // sidebar's 200px minimum the row's content box is 168px (200 minus its
    // own 32px padding), and the live-measured wordmark advance (~62px) +
    // pill floor 44 + two 28px buttons + the 2px button margin = ~164px ≤
    // 168. The pill floor of 44 keeps "Search" (~35px at --type-meta) inside
    // the 44 - 4 - 2 = 38px of content the pill's padding and border leave.
    inject([".sidebar-top", ".sidebar-search", ".sidebar-search input", ".sidebar-foot"]);
    await renderWorkspace();

    const top = document.querySelector<HTMLElement>(".sidebar-top");
    if (top === null) throw new Error("sidebar top did not render");
    expect(getComputedStyle(top).gap).toBe("0");

    const pill = document.querySelector<HTMLElement>(".sidebar-search");
    if (pill === null) throw new Error("search pill did not render");
    expect(getComputedStyle(pill).minWidth).toBe("44px");
    const input = pill.querySelector("input");
    if (input === null) throw new Error("search input did not render");
    expect(getComputedStyle(input).paddingLeft).toBe("0px");
    expect(getComputedStyle(input).paddingRight).toBe("0px");

    const foot = document.querySelector<HTMLElement>(".sidebar-foot");
    if (foot === null) throw new Error("daemon foot did not render");
    expect(getComputedStyle(foot).gap).toBe("8px");
  });

  it("the foot's rows sit on the body rows' 16px column", async () => {
    // The composed left edge is what the eye sees: container padding plus
    // the row's own padding must add up to the same 16px column the body
    // rows start at. (The 994f6c1 pass shipped a 2px footer container, which
    // put the Daemon row at 10px while body text sat at 16px.)
    inject([
      ".sidebar-body",
      ".workspace-row",
      ".workspace-sidebar-footer",
      ".workspace-history-button",
      ".sidebar-foot",
    ]);
    await renderWorkspace();

    const px = (el: HTMLElement, prop: "paddingLeft" | "paddingRight") =>
      Number.parseFloat(getComputedStyle(el)[prop]);

    const body = document.querySelector<HTMLElement>(".sidebar-body");
    const row = document.querySelector<HTMLElement>(".workspace-row");
    const footer = document.querySelector<HTMLElement>(".workspace-sidebar-footer");
    const historyButton = document.querySelector<HTMLElement>(".workspace-history-button");
    const daemonRow = document.querySelector<HTMLElement>(".sidebar-foot");
    for (const el of [body, row, footer, historyButton, daemonRow]) {
      if (el === null) throw new Error("a sidebar column element did not render");
    }

    // eslint-disable-next-line no-console
    console.log(
      "DEBUG cols:",
      px(body!, "paddingLeft"),
      px(row!, "paddingLeft"),
      px(footer!, "paddingLeft"),
      px(historyButton!, "paddingLeft"),
      px(daemonRow!, "paddingLeft"),
    );
    const bodyColumn = px(body!, "paddingLeft") + px(row!, "paddingLeft");
    const historyColumn = px(footer!, "paddingLeft") + px(historyButton!, "paddingLeft");
    const daemonColumn = px(footer!, "paddingLeft") + px(daemonRow!, "paddingLeft");

    expect(bodyColumn).toBe(16);
    expect(historyColumn).toBe(bodyColumn);
    expect(daemonColumn).toBe(bodyColumn);
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
