// @vitest-environment happy-dom

// Stacking of every modal in the inventory against the real stylesheets:
// the assembled sheets in the bundle's own order (tokens, global, then the
// surface sheets). The selectors this file reads never share a rule, so the
// order between the surface sheets changes none of these assertions — noted
// here so nobody "fixes" the list into a fragile order.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../features/workspace/cssProof";

const rootDir = resolve(import.meta.dirname, "../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

const SHEETS = [
  read("src/styles/tokens.css"),
  read("src/styles/global.css"),
  read("src/features/workspace/Workspace.css"),
  read("src/features/settings/profiles.css"),
  read("src/features/design/design.css"),
];

const CRESCENT_Z = 40;

function zIndexOf(rulesFor: (selector: string) => string, selector: string): number {
  const match = rulesFor(selector).match(/z-index:\s*(-?\d+)/);
  if (match === null) throw new Error(`${selector} declares no z-index`);
  return Number(match[1]);
}

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe("the page layer traps nothing (real stylesheet)", () => {
  const { rulesFor } = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/design/design.css"),
  ]);

  // Source checks, stated as ones: a stacking context is created by a
  // positioned element with a z-index, by a transform, or by an opacity
  // under 1 — so the rule that declares none of the three creates none.
  // (Computed transform reads as an empty string under happy-dom, which
  // would make a computed assertion engine-dependent for no gain.)
  it.each([
    [".page-layer", "the page layer"],
    [".app-shell", "the shell"],
    [".design-surface", "the design surface"],
    [".design-toolbar", "the design toolbar"],
    [".design-skill-controls", "the craft mode controls"],
    [".design-history-menu", "the history menu"],
  ] as const)("declares no stacking-context creator on %s", (selector, name) => {
    const body = rulesFor(selector);
    expect(body, `${name} z-index`).not.toContain("z-index:");
    expect(body, `${name} transform`).not.toContain("transform");
    expect(body, `${name} opacity`).not.toContain("opacity");
  });
});

describe("every inventoried modal outranks the crescent (real stylesheets)", () => {
  const { rulesFor } = assembleCssProof(SHEETS);

  it.each([
    [".workspace-project-dialog-backdrop", "New project"],
    [".edit-scrim", "Profile"],
    [".design-agent-picker", "Design skill picker"],
    [".design-craft-overlay", "Design craft sheet"],
    [".design-history-popover", "Design history popover"],
  ] as const)("the %s modal paints above the crescent band", (selector, name) => {
    expect(zIndexOf(rulesFor, selector), name).toBeGreaterThan(CRESCENT_Z);
  });

  it("the crescent band is the 40 the modals must beat", () => {
    expect(zIndexOf(rulesFor, ".crescent-shell")).toBe(CRESCENT_Z);
  });

  it("the nav-open shade sits between the page and the crescent", () => {
    const shade = zIndexOf(rulesFor, ".page-dim");
    expect(shade).toBeGreaterThan(0);
    expect(shade).toBeLessThan(CRESCENT_Z);
  });
});
