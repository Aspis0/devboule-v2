// @vitest-environment happy-dom

// Styling proof for the sidebar slice without launching the app. The REAL
// stylesheets are parsed here (comments stripped, brace-matched, @-blocks
// skipped), the rules the sidebar depends on are extracted from them, their
// tokens are resolved to the light values, and those exact rules are injected
// into the document so the computed styles of the rendered chrome can be
// asserted. A rule swallowed by a malformed comment (the live defect) or a
// selector that never matches the markup fails these assertions.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { beforeEachHarness, renderWorkspace, unmountWorkspace } from "../bulkCloseHarness";

const rootDir = resolve(import.meta.dirname, "../../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

const withoutComments = (css: string) => css.replace(/\/\*[\s\S]*?\*\//g, "");

interface CssRule {
  selector: string;
  body: string;
}

// Walks the sheet with brace matching; @-blocks (media/keyframes/fonts) are
// skipped whole.
function parseRules(css: string): CssRule[] {
  const rules: CssRule[] = [];
  let index = 0;
  while (index < css.length) {
    const open = css.indexOf("{", index);
    if (open < 0) break;
    const selector = css.slice(index, open).trim();
    const close = css.indexOf("}", open);
    if (close < 0) break;
    const body = css.slice(open + 1, close);
    if (selector.startsWith("@")) {
      let depth = 1;
      let cursor = open + 1;
      while (depth > 0 && cursor < css.length) {
        if (css[cursor] === "{") depth += 1;
        if (css[cursor] === "}") depth -= 1;
        cursor += 1;
      }
      index = cursor;
      continue;
    }
    rules.push({ selector: selector.replace(/\s+/g, " "), body });
    index = close + 1;
  }
  return rules;
}

function selectorMatches(ruleSelector: string, target: string): boolean {
  return ruleSelector
    .split(",")
    .map((part) => part.trim())
    .some((part) => part === target);
}

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await unmountWorkspace();
  document.querySelectorAll("style[data-sidebar-proof]").forEach((el) => el.remove());
});

describe("sidebar computed styles (real stylesheets, no app launch)", () => {
  // Sheet order matches the bundle: tokens, global, workspace, sidebar.
  const stripped = [
    withoutComments(read("src/styles/tokens.css")),
    withoutComments(read("src/styles/global.css")),
    withoutComments(read("src/features/workspace/Workspace.css")),
    withoutComments(read("src/features/workspace/sidebar/sidebar.css")),
  ];

  // Light-theme custom properties, for resolving var() before injection.
  const tokens = new Map<string, string>();
  for (const sheet of stripped) {
    for (const block of sheet.matchAll(/:root\s*\{([^}]*)\}/g)) {
      for (const m of block[1]!.matchAll(/--([a-zA-Z0-9-]+):\s*([^;]+);/g)) {
        tokens.set(`--${m[1]!.trim()}`, m[2]!.trim());
      }
    }
  }
  const resolveVars = (css: string): string => {
    let current = css;
    for (let pass = 0; pass < 4; pass += 1) {
      current = current.replace(
        /var\((--[a-zA-Z0-9-]+)\)/g,
        (whole: string, name: string) => tokens.get(name) ?? whole,
      );
    }
    return current;
  };

  const allRules = parseRules(resolveVars(stripped.join("\n")));

  function rulesFor(target: string): string {
    return allRules
      .filter((rule) => selectorMatches(rule.selector, target))
      .map((rule) => rule.body)
      .join("\n");
  }

  function inject(targets: readonly string[]): void {
    const picked = allRules.filter((rule) =>
      targets.some((target) => selectorMatches(rule.selector, target)),
    );
    const style = document.createElement("style");
    style.setAttribute("data-sidebar-proof", "");
    style.textContent = picked.map((rule) => `${rule.selector} { ${rule.body} }`).join("\n");
    document.head.appendChild(style);
  }

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

  it("the resize handle keeps its 6px track and col-resize cursor", async () => {
    inject([".workspace-resize-handle"]);
    await renderWorkspace();
    const handle = document.querySelector<HTMLElement>(".workspace-resize-handle");
    if (handle === null) throw new Error("resize handle did not render");
    const style = getComputedStyle(handle);
    expect(style.width).toBe("6px");
    expect(style.cursor).toBe("col-resize");
  });

  it("the tab strip is a flex row again, not a stacked block", async () => {
    inject([".workspace-session-tabs"]);
    await renderWorkspace();
    const strip = document.querySelector<HTMLElement>(".workspace-session-tabs");
    if (strip === null) throw new Error("session tab strip did not render");
    const style = getComputedStyle(strip);
    expect(style.display).toBe("flex");
    expect(style.height).toBe("44px");
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
