// The composer slice's mono-label contract (SPEC principle 2): inside the
// families this slice owns — the composer, the chat-label, the subagent pill
// and rows, the picker chips and the single-model label — JetBrains Mono may
// not appear. The walk reads the real stylesheets through cssProof and checks
// every mono-declaring rule it finds: a rule passes only if every selector
// part is a command chip (the spec's mono vocabulary) or belongs to a family
// the slice does not own. The policy lives here; the parsing lives in
// cssProof, so a NEW mono rule under an owned prefix fails the walk — the
// thing a hand-written target list cannot catch. Both themes are walked:
// a dark-theme override of an owned selector is just as much a violation.
//
// The rest of the file pins the slice's own values: the deleted labels are
// gone outright, the restyled labels are the UI font at 12 px or above, and
// the focus cue, the menu's max-width and the text alignment are as built.
// The picker menu's option rows are sized in `src/components/PickerChip.test.tsx`
// (look-calls slice), so this file walks them for mono only, never for size.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { assembleCssProof } from "./cssProof";

const rootDir = resolve(import.meta.dirname, "../../..");
// tokens.css first, so var() in the owned sheets resolves the way the bundle
// resolves it; the walk then also sees the token definitions themselves.
const OWNED_SHEETS = [
  readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/features/workspace/Workspace.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/features/workspace/SubagentMenu.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/components/PickerChip.css"), "utf8"),
];

// The selector families this slice owns, at the start of a selector or
// behind a descendant space. Anything else in Workspace.css (trees, tool
// rows, dialogs, banners) belongs to another slice's block and is not this
// test's scope.
const OWNED_FAMILY = /(^|\s)\.workspace-(composer|chat-label|subagent|mode-|picker-static)\b/;
// Command chips are the spec's one mono UI element. The walk's policy is
// the owned families: a mono rule fails only when one of its selector
// parts targets one. The command family sits outside them, so this
// exemption changes no verdict — it records where mono is legal.
const COMMAND_CHIP = /^\.workspace-command-/;

// The labels the slice deleted outright: the provider manifest line above
// the transcript. Their rules must be gone, not merely restyled — a restyled
// mono label would still fail the walk above.
const DELETED_SELECTORS = [
  ".workspace-agent-manifest",
  ".workspace-agent-manifest select",
  ".workspace-agent-manifest select:hover",
  ".workspace-agent-manifest-pending",
];

describe("the owned families declare no mono, in both themes", () => {
  it.each(["light", "dark"] as const)(
    "%s: every mono rule is a command chip or outside the owned families",
    (theme) => {
      const css = assembleCssProof(OWNED_SHEETS, theme);
      const mono = css.monoDeclarations;
      expect(mono.length).toBeGreaterThan(0); // the sheets do declare mono somewhere
      for (const rule of mono) {
        for (const part of rule.selector.split(",").map((selector) => selector.trim())) {
          if (COMMAND_CHIP.test(part)) continue;
          expect(OWNED_FAMILY.test(part), `mono declared by \`${rule.selector}\``).toBe(false);
        }
      }
    },
  );
});

describe("the deleted labels are gone", () => {
  it.each(DELETED_SELECTORS)("%s has no rule left", (selector) => {
    const css = assembleCssProof(OWNED_SHEETS);
    expect(css.rulesFor(selector)).toBe("");
  });
});

describe("the restyled labels speak the UI font at 12px or above", () => {
  const UI_FONT_AT_12 = [
    ".workspace-chat-label",
    ".workspace-subagent-pill",
    ".workspace-mode-chip-trigger",
    ".workspace-picker-static", // the static single-model label
    ".workspace-composer-hint",
  ];

  it.each(UI_FONT_AT_12)("%s is the UI font at 12px", (selector) => {
    const css = assembleCssProof(OWNED_SHEETS);
    const rules = css.rulesFor(selector);
    expect(rules).toContain('font-family: "Inter", system-ui, sans-serif');
    expect(rules).toContain("font-size: 12px");
  });

  it("the subagent rows are the UI font at the mockup's 13px", () => {
    // The mockup's row is 13 px (skeleton .sub-row) — above the 12 px floor
    // this describe block enforces, so it is pinned at its own value.
    const css = assembleCssProof(OWNED_SHEETS);
    const rules = css.rulesFor(".workspace-subagent-row");
    expect(rules).toContain('font-family: "Inter", system-ui, sans-serif');
    expect(rules).toContain("font-size: 13px");
  });

  it("the composer text is the spec's 14/1.45 on the transcript column", () => {
    const css = assembleCssProof(OWNED_SHEETS);
    const rules = css.rulesFor(".workspace-composer");
    expect(rules).toContain("padding: 14px 24px 12px");
    expect(css.rulesFor(".workspace-composer textarea")).toContain("font-size: 14px");
    expect(css.rulesFor(".workspace-composer textarea")).toContain("line-height: 1.45");
  });

  it("the chat-label and the empty line are sentence case, not uppercased labels", () => {
    const css = assembleCssProof(OWNED_SHEETS);
    for (const selector of [".workspace-chat-label", ".workspace-chat-empty"]) {
      const rules = css.rulesFor(selector);
      expect(rules).not.toContain("text-transform");
      expect(rules).not.toContain("letter-spacing");
      expect(rules).toContain('font-family: "Inter", system-ui, sans-serif');
      expect(rules).toContain("font-size: 12px");
    }
  });
});

describe("the composer chrome's pinned values", () => {
  it("keeps a 2px focus ring on the card, not just the 1px border move", () => {
    const css = assembleCssProof(OWNED_SHEETS);
    const rules = css.rulesFor(".workspace-composer:focus-within");
    // Resolved to the accent's rgb triplet by the proof, as the bundle does.
    expect(rules).toContain("box-shadow: 0 0 0 2px rgba(189, 74, 38, 0.22)");
  });

  it("caps the command menu at the wrap's content box, not its padding box", () => {
    const css = assembleCssProof(OWNED_SHEETS);
    const rules = css.rulesFor(".workspace-command-menu");
    expect(rules).toContain("max-width: calc(100% - (24px * 2))");
    expect(rules).not.toContain("max-width: 100%");
  });

  it("carries no shadow token: --shadow-card is none in both themes", () => {
    const css = assembleCssProof(OWNED_SHEETS);
    expect(css.rulesFor(".workspace-composer")).not.toContain("box-shadow");
  });
});
