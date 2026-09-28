// @vitest-environment happy-dom

// Stacking of every dialog against the real stylesheets, walked from the
// rendered DOM: each dialog is mounted for real and the walk reads its actual
// ancestor chain — no hand-written list of selectors — checking that no
// ancestor creates a stacking context (which would trap the dialog under the
// crescent band) and that the dialog's own z-index beats the band's. The
// sheets are every stylesheet under src.
//
// Scope, stated plainly: the proof helper skips @-blocks and descendant
// selectors wholesale, so an ancestor's transform or z-index declared in a
// media query or a descendant rule is invisible here — the walk proves the
// chains the sheets actually declare, not every chain they could declare.
// Computed values come from the proof helper's inject: happy-dom reads a
// property as "" when no injected rule declares it, so "" is the answer to
// "no creator here".
//
// The second describe pins the one modal dim: --modal-dim carries the
// dialogs' 0.34 in light and black 0.5 in dark, and the profile scrim plus
// the workspace dialog backdrops paint from the token — the raw sheets must
// name it, so a copied literal cannot pass. The confirm dialog still carries
// its own dim; it belongs to another slice (noted in
// CODER-REPORT-look-calls.md) and is not pinned here.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { act } from "react";
import type { ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../features/plugins/install", () => ({ chooseAndInstall: vi.fn() }));
vi.mock("../features/design/DesignHistoryList", () => ({ DesignHistoryList: () => null }));

import { assembleCssProof, removeCssProof } from "../features/workspace/cssProof";
import { ANCHORED_POPOVER_Z_INDEX } from "../features/workspace/popoverPlace";
import { NewProjectDialog } from "../components/NewProjectDialog";
import { ProfileDialog } from "../features/settings/profiles/ProfileDialog";
import { buildSkillBlock } from "../features/design/skillLoader";
import type { BuiltInSkillIndexEntry } from "../features/design/builtInSkills";
import type { DesignSkillSelection } from "../features/design/designSettings";
import {
  DesignCraftSheet,
  DesignSkillModeControl,
  DesignToolbar,
} from "../features/design/DesignSurface";
import { DesignFolderControl } from "../features/design/DesignFolderControl";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { ContextPopover } from "../features/workspace/ContextPopover";

const rootDir = resolve(import.meta.dirname, "../..");

function allSheets(): string[] {
  const walk = (dir: string): string[] =>
    readdirSync(dir).flatMap((entry) => {
      const path = join(dir, entry);
      if (statSync(path).isDirectory()) return walk(path);
      return entry.endsWith(".css") ? [readFileSync(path, "utf8")] : [];
    });
  return walk(join(rootDir, "src"));
}

const proof = assembleCssProof(allSheets());

function lastZIndex(rules: string): number {
  const matches = [...rules.matchAll(/z-index:\s*(-?\d+)/g)];
  if (matches.length === 0) throw new Error("rule declares no z-index");
  return Number(matches[matches.length - 1]![1]);
}

/** The crescent band's own z-index, read from the sheets it is declared in. */
const CRESCENT_Z = (() => {
  const rules = proof.rulesFor(".crescent-shell");
  expect(rules).not.toBe("");
  return lastZIndex(rules);
})();

async function mountDialog(
  node: ReactNode,
  find: (root: ParentNode) => Element | null,
  open?: (container: HTMLDivElement) => Promise<void>,
): Promise<{ container: HTMLDivElement; root: ReturnType<typeof createRoot>; start: Element }> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(node);
  });
  // Portaled surfaces render into document.body, outside the container.
  if (open !== undefined) await open(container);
  const start = find(document);
  if (start === null) throw new Error("dialog element did not render");
  return { container, root, start };
}

/** Every ancestor of `start`, from the element itself to <html>. */
function walkUp(start: Element): Element[] {
  const chain: Element[] = [];
  let current: Element | null = start;
  while (current !== null) {
    chain.push(current);
    current = current.parentElement;
  }
  return chain;
}

function classesOf(element: Element): string[] {
  return [...element.classList];
}

/**
 * The chain's computed styles, after the real rules for every class and id
 * on it are injected — so each read is the cascade's own answer, and a class
 * nobody styled reads as undeclared.
 */
function computedChain(start: Element): { classes: string[]; style: CSSStyleDeclaration }[] {
  const chain = walkUp(start);
  proof.inject(
    chain.flatMap((element) => [
      ...classesOf(element).map((cls) => `.${cls}`),
      ...(element.id === "" ? [] : [`#${element.id}`]),
    ]),
  );
  return chain.map((element) => ({
    classes: classesOf(element),
    style: getComputedStyle(element),
  }));
}

/**
 * The shared assertion for one dialog: its z-index beats the band's, and
 * nothing on its ancestor chain creates a stacking context. Every class on
 * the chain must also resolve to a real rule — a renamed or dropped class
 * would otherwise turn the creator checks into tautologies.
 */
function expectUnbornByTheBand(chain: { classes: string[]; style: CSSStyleDeclaration }[]): void {
  for (const { classes } of chain) {
    for (const cls of classes) {
      expect(proof.rulesFor(`.${cls}`), `${cls} resolves to a real rule`).not.toBe("");
    }
  }
  for (const { classes, style } of chain.slice(1)) {
    const label = classes.join(".") || "(unclassed)";
    expect(style.transform, `${label} transform`).toBe("");
    expect(style.opacity, `${label} opacity`).toBe("");
    expect(style.filter, `${label} filter`).toBe("");
    expect(style.contain, `${label} contain`).toBe("");
    expect(style.perspective, `${label} perspective`).toBe("");
    expect(style.willChange, `${label} will-change`).toBe("");
    const rules = classes.map((cls) => proof.rulesFor(`.${cls}`)).join("\n");
    expect(rules, `${label} backdrop-filter`).not.toContain("backdrop-filter");
    expect(rules, `${label} isolation`).not.toContain("isolation");
    expect(rules, `${label} mix-blend-mode`).not.toContain("mix-blend-mode");
    // A z-index creates a context only on a positioned box.
    if (style.position !== "" && style.position !== "static") {
      expect(["auto", ""], `${label} z-index on a positioned box`).toContain(style.zIndex);
    }
  }
}

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

function ConfirmDialogStandalone() {
  return (
    <ConfirmDialog
      open
      title="Close tab"
      message="3 unsaved changes?"
      confirmLabel="Close tab"
      tone="danger"
      onConfirm={() => undefined}
      onCancel={() => undefined}
    />
  );
}

describe("the page layer and the band (real stylesheets, computed)", () => {
  it("the page layer creates no stacking context", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        <div className="app-shell">
          <div className="page-layer" />
        </div>,
      );
    });
    proof.inject([".app-shell", ".page-layer"]);
    const layer = document.querySelector(".page-layer")!;
    const style = getComputedStyle(layer);
    expect(style.transform).toBe("");
    expect(style.opacity).toBe("");
    expect(style.filter).toBe("");
    expect(style.position).toBe("relative");
    // Relative without a z-index is not a context.
    expect(style.zIndex).toBe("");
    await act(async () => root.unmount());
  });

  it("the nav-open shade sits between the page and the crescent", () => {
    const rules = proof.rulesFor(".page-dim");
    expect(rules).not.toBe("");
    const z = lastZIndex(rules);
    expect(z).toBeGreaterThan(0);
    expect(z).toBeLessThan(CRESCENT_Z);
  });

  it("the design shell overrides the base transition, so its slide never animates margin-top", () => {
    const override = proof.rulesFor(".app-shell-design .page-layer");
    expect(override).not.toBe("");
    expect(override).toContain("height");
    expect(override).not.toContain("margin-top");
  });
});

describe("every dialog outranks the crescent and no ancestor traps it", () => {
  it("New project", async () => {
    const { root, start } = await mountDialog(
      <NewProjectDialog open onClose={() => undefined} onCreate={() => undefined} />,
      (container) => container.querySelector(".workspace-project-dialog-backdrop"),
    );
    const chain = computedChain(start);
    expectUnbornByTheBand(chain);
    expect(Number(chain[0]!.style.zIndex)).toBeGreaterThan(CRESCENT_Z);
    await act(async () => root.unmount());
  });

  it("Profile — and its scrim's containing block is the viewport, so it covers the band", async () => {
    const { root, start } = await mountDialog(
      <ProfileDialog open title="Edit profile" busy={false} onClose={() => undefined}>
        {() => (
          <form>
            <input aria-label="Profile name" />
          </form>
        )}
      </ProfileDialog>,
      (container) => container.querySelector(".edit-scrim"),
    );
    const scrimRules = proof.rulesFor(".edit-scrim");
    expect(scrimRules).toContain("position: fixed");
    const chain = computedChain(start);
    expectUnbornByTheBand(chain);
    // A fixed box whose every ancestor creates no containing block takes the
    // initial containing block — the viewport — which is what lets the scrim
    // cover the crescent band while the dialog is open.
    expect(Number(chain[0]!.style.zIndex)).toBeGreaterThan(CRESCENT_Z);
    await act(async () => root.unmount());
  });

  it("the Design skill picker", async () => {
    const skillSelection: DesignSkillSelection = { version: 1, mode: "all", enabledSlugs: [] };
    const { root, start } = await mountDialog(
      <DesignSkillModeControl
        skillSelection={skillSelection}
        onSkillModeChange={() => undefined}
        onCraftOpen={() => undefined}
        onCraftReadMore={() => undefined}
      />,
      (root) => root.querySelector("#design-skill-picker"),
      async (container) => {
        const trigger = container.querySelector<HTMLButtonElement>(
          '[data-design-skill-mode-trigger="true"]',
        );
        if (trigger === null) throw new Error("craft mode trigger did not render");
        await act(async () => trigger.click());
      },
    );
    const chain = computedChain(start);
    expectUnbornByTheBand(chain);
    expect(Number(chain[0]!.style.zIndex)).toBeGreaterThan(CRESCENT_Z);
    await act(async () => root.unmount());
  });

  it("the Design craft sheet", async () => {
    const skillSelection: DesignSkillSelection = { version: 1, mode: "manual", enabledSlugs: [] };
    const { root, start } = await mountDialog(
      <DesignCraftSheet
        open
        skillIndex={[] as readonly BuiltInSkillIndexEntry[]}
        skillSelection={skillSelection}
        selectedSkillSlugs={[]}
        resolvedSkillSlugs={null}
        appliedSkillSlugs={null}
        hasResolvedComposition={false}
        skillBlock={buildSkillBlock([], [])}
        resolvedSkillSlugSet={new Set()}
        automaticBaselineSlugSet={new Set()}
        droppedSkillSlugSet={new Set()}
        readOnly={false}
        onClose={() => undefined}
        onSkillToggle={() => undefined}
      />,
      (container) => container.querySelector(".design-craft-overlay"),
    );
    const chain = computedChain(start);
    expectUnbornByTheBand(chain);
    expect(Number(chain[0]!.style.zIndex)).toBeGreaterThan(CRESCENT_Z);
    await act(async () => root.unmount());
  });

  it("the Design history popover", async () => {
    const { root, start } = await mountDialog(
      <DesignToolbar
        folderControl={null}
        grounded
        outputMode="page"
        busy={false}
        onOutputModeChange={() => undefined}
        canSave={false}
        saved
        saving={false}
        saveError={null}
        canUndo={false}
        canRedo={false}
        historyRefreshKey={0}
        liveSessionId={null}
        onGroundingToggle={() => undefined}
        onSave={() => undefined}
        onUndo={() => undefined}
        onRedo={() => undefined}
        onHistoryOpen={() => true}
      />,
      (container) => container.querySelector("#design-history-popover"),
    );
    const chain = computedChain(start);
    expectUnbornByTheBand(chain);
    expect(Number(chain[0]!.style.zIndex)).toBeGreaterThan(CRESCENT_Z);
    await act(async () => root.unmount());
  });

  it("the portaled confirm dialog", async () => {
    // The backdrop, not the card: the card's own box carries no z-index —
    // the backdrop's does, and it is the box the band would have to beat.
    const { root, start } = await mountDialog(<ConfirmDialogStandalone />, (root) =>
      root.querySelector(".confirm-dialog-backdrop"),
    );
    const chain = computedChain(start);
    expectUnbornByTheBand(chain);
    expect(Number(chain[0]!.style.zIndex)).toBeGreaterThan(CRESCENT_Z);
    // The destructive ask must also paint over the anchored popovers, whose
    // layer is the inline number in popoverPlace — read here as the constant
    // both places share, so a renumber on either side fails loudly.
    expect(Number(chain[0]!.style.zIndex)).toBeGreaterThan(ANCHORED_POPOVER_Z_INDEX);
    await act(async () => root.unmount());
  });

  it("the portaled context popover", async () => {
    const { root, start } = await mountDialog(
      <ContextPopover
        open
        anchorRef={{ current: null }}
        onClose={() => undefined}
        numbers={{ used: 10, max: 100, percent: 10 }}
        live={false}
        plan={null}
      />,
      (container) => container.querySelector(".workspace-context-popover"),
    );
    const chain = computedChain(start);
    expectUnbornByTheBand(chain);
    expect(Number(chain[0]!.style.zIndex)).toBeGreaterThan(CRESCENT_Z);
    await act(async () => root.unmount());
  });

  it("the Design folder picker — a second class on the element, not its own rule", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        <DesignFolderControl
          folders={[]}
          loading={false}
          refreshing={false}
          foldersError={null}
          selectionNotice={null}
          selectedWorkspaceId={null}
          selectionUnresolved={false}
          attachedPath={null}
          disabled={false}
          attachBusy={false}
          attachError={null}
          onOpen={() => undefined}
          onSelect={() => undefined}
          onAttach={async () => false}
          onUseFolder={async () => false}
        />,
      );
    });
    const trigger = container.querySelector<HTMLButtonElement>(
      '[data-design-folder-trigger="true"]',
    );
    if (trigger === null) throw new Error("folder trigger did not render");
    await act(async () => trigger.click());
    const start = container.querySelector("#design-folder-picker");
    if (start === null) throw new Error("folder picker did not open");
    // The picker's z-index comes from the shared .design-agent-picker class;
    // a rule added to .design-folder-picker itself would be read too, because
    // the walk injects every class on the element.
    expect(proof.rulesFor(".design-folder-picker")).not.toBe("");
    const chain = computedChain(start);
    expectUnbornByTheBand(chain);
    expect(Number(chain[0]!.style.zIndex)).toBeGreaterThan(CRESCENT_Z);
    await act(async () => root.unmount());
  });
});

describe("the four owned modal backdrops share the one dim", () => {
  const DIM = "rgba(28, 26, 23, 0.34)";
  const DARK_DIM = "rgba(0, 0, 0, 0.5)";
  const BACKDROPS = [
    ".edit-scrim",
    ".workspace-project-dialog-backdrop",
    ".workspace-rename-backdrop",
    ".confirm-dialog-backdrop",
  ];
  const darkProof = assembleCssProof(allSheets(), "dark");

  it("defines --modal-dim per theme: the dialogs' 0.34 in light, black 0.5 in dark", () => {
    expect(proof.token("--modal-dim")).toBe(DIM);
    expect(darkProof.token("--modal-dim")).toBe(DARK_DIM);
  });

  it("paints the profile scrim, the workspace dialogs and the confirm dialog from the token", () => {
    for (const selector of BACKDROPS) {
      expect(proof.rulesFor(selector), `${selector} backdrop in light`).toContain(
        `background: ${DIM}`,
      );
      expect(darkProof.rulesFor(selector), `${selector} backdrop in dark`).toContain(
        `background: ${DARK_DIM}`,
      );
    }
    // rulesFor reads the token already resolved, so it proves the value, not
    // the spelling: the raw sheets below prove the backdrops name the token.
    const workspace = readFileSync(
      resolve(rootDir, "src/features/workspace/Workspace.css"),
      "utf8",
    );
    const profiles = readFileSync(resolve(rootDir, "src/features/settings/profiles.css"), "utf8");
    expect(workspace).toContain("background: var(--modal-dim)");
    expect(workspace).not.toContain("rgba(var(--ink-rgb), 0.34)");
    expect(profiles).toContain("background: var(--modal-dim)");
    expect(profiles).not.toContain("background: var(--scrim)");
    const confirm = readFileSync(resolve(rootDir, "src/components/ConfirmDialog.css"), "utf8");
    expect(confirm).toContain("background: var(--modal-dim)");
    expect(confirm).not.toContain("rgba(var(--ink-rgb), 0.34)");
  });
});
