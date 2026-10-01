// The slice's weight map: structural labels and metadata paint at or below 500,
// Markdown headings and table headers keep 600. Most pins read the rule source;
// the plan pill's two rows are pinned in the computed cascade, where their
// head's 500 would otherwise win.
// @vitest-environment happy-dom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../features/workspace/cssProof";

const ROOT = resolve(import.meta.dirname, "../..");
const read = (rel: string) => readFileSync(resolve(ROOT, rel), "utf8");

function declaredWeights(css: string, selector: string): number[] {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  // A trailing :hover or [attribute] arm belongs to the same selector: match
  // it too, so a heavier state cannot slip past the gate unnoticed.
  const suffix = String.raw`(?:::[a-zA-Z-]+|:not\([^)]*\)|:[a-zA-Z-]+(?:\([^)]*\))?|\[[^\]]*\])*`;
  const bodies = [...css.matchAll(new RegExp(`${escaped}${suffix}\\s*\\{([^}]*)\\}`, "gs"))].map(
    (match) => match[1]!,
  );
  expect(bodies.length, `${selector} has no rule`).toBeGreaterThan(0);
  // Follow-up rules may only add padding or layout without touching weight;
  // every rule that does declare a weight must satisfy the caller. A later
  // duplicate 600 on a 500 label fails here through the maximum below.
  // A var weight resolves through the shared :root token, the sheets' own
  // convention for label weight.
  const weights = bodies.flatMap((body) => {
    const match = /font-weight:\s*(?:(\d+)|var\((--[\w-]+)\))/.exec(body);
    if (match?.[1] !== undefined) return [Number.parseInt(match[1], 10)];
    if (match?.[2] !== undefined) return [sharedTokenWeight(match[2])];
    return [];
  });
  expect(weights.length, `${selector} declares no numeric font-weight`).toBeGreaterThan(0);
  return weights;
}

describe("workspace chrome labels", () => {
  const css = () => read("src/features/workspace/Workspace.css");
  for (const selector of [
    ".workspace-vertical-label",
    ".workspace-chat-label",
    ".workspace-menu-label",
    ".workspace-consent-provider .workspace-surface-name",
    ".workspace-generation-heading",
  ]) {
    it(`${selector} paints at or below 500`, () => {
      for (const weight of declaredWeights(css(), selector)) {
        expect(weight).toBeLessThanOrEqual(500);
      }
    });
  }

  for (const selector of [
    ".workspace-terminal-toolbar .workspace-terminal-title",
    ".workspace-agent-toolbar .workspace-agent-title",
  ]) {
    it(`${selector} paints at or below 500`, () => {
      const pane = read("src/features/workspace/paneHeader/paneHeader.css");
      for (const weight of declaredWeights(pane, selector)) {
        expect(weight).toBeLessThanOrEqual(500);
      }
    });
  }
});

describe("panel and diff metadata", () => {
  it(".workspace-panel-empty-title paints at or below 500", () => {
    const css = read("src/features/workspace/panel/panel.css");
    for (const weight of declaredWeights(css, ".workspace-panel-empty-title")) {
      expect(weight).toBeLessThanOrEqual(500);
    }
  });

  it("hunk rows paint at or below 500", () => {
    const changes = read("src/features/workspace/panel/changes.css");
    const diffTab = read("src/features/workspace/panel/diffTab.css");
    for (const weight of declaredWeights(changes, ".workspace-diff-hunk")) {
      expect(weight).toBeLessThanOrEqual(500);
    }
    for (const weight of declaredWeights(diffTab, ".diff-tab-hunk")) {
      expect(weight).toBeLessThanOrEqual(500);
    }
  });
});

describe("card and control labels", () => {
  it("permission card action and question paint at or below 500", () => {
    const css = read("src/components/PermissionCard.css");
    for (const weight of declaredWeights(css, ".permission-card > .permission-card-action")) {
      expect(weight).toBeLessThanOrEqual(500);
    }
    for (const weight of declaredWeights(css, ".permission-card-question-text")) {
      expect(weight).toBeLessThanOrEqual(500);
    }
  });

  it("the selected mode name and the policy banner title paint at or below 500", () => {
    const picker = read("src/components/PickerChip.css");
    const banner = read("src/features/settings/providers.css");
    for (const weight of declaredWeights(
      picker,
      '.workspace-mode-option[aria-selected="true"] .workspace-mode-name',
    )) {
      expect(weight).toBeLessThanOrEqual(500);
    }
    for (const weight of declaredWeights(banner, ".prov-policy-banner-title")) {
      expect(weight).toBeLessThanOrEqual(500);
    }
  });
});

describe("markdown headings keep 600", () => {
  it("transcript headings stay at 600", () => {
    const css = read("src/features/workspace/timeline/timeline.css");
    for (const selector of [
      ".workspace-chat-assistant .workspace-chat-copy .plan-markdown-heading-1",
      ".workspace-chat-assistant .workspace-chat-copy .plan-markdown-heading-2",
      ".workspace-chat-assistant .workspace-chat-copy .plan-markdown-heading-3",
      ".workspace-chat-assistant .workspace-chat-copy .plan-markdown-heading-4",
    ]) {
      for (const weight of declaredWeights(css, selector)) {
        expect(weight).toBe(600);
      }
    }
  });

  it("file-tab preview headings stay at 600", () => {
    const css = read("src/features/workspace/fileTab.css");
    for (const selector of [
      ".workspace-file-tab-preview .plan-markdown-heading-1",
      ".workspace-file-tab-preview .plan-markdown-heading-2",
      ".workspace-file-tab-preview .plan-markdown-heading-3",
    ]) {
      for (const weight of declaredWeights(css, selector)) {
        expect(weight).toBe(600);
      }
    }
  });

  it("markdown table headers stay at 600", () => {
    const css = read("src/components/markdown.css");
    for (const weight of declaredWeights(css, ".plan-markdown-table th")) {
      expect(weight).toBe(600);
    }
  });
});

const PILL_SHEETS = ["src/styles/tokens.css", "src/features/workspace/AgentTaskPill.css"];

// Label weight lives in one shared token; a var weight resolves through it.
function sharedTokenWeight(name: string): number {
  const root = /:root\s*\{([^}]*)\}/.exec(read("src/styles/tokens.css"))?.[1] ?? "";
  const value = new RegExp(`${name}:\\s*(\\d+)`).exec(root)?.[1];
  expect(value, `${name} must stay a numeric shared token`).toBeTruthy();
  return Number.parseInt(value!, 10);
}

describe("the plan pill's rows", () => {
  afterEach(removeCssProof);

  it.each(["light", "dark"] as const)("read 400 under the head's 500 (%s)", (theme) => {
    const css = assembleCssProof(PILL_SHEETS.map(read), theme);
    css.inject([".agent-task-pill-head", ".agent-task-pill-current", ".agent-task-pill-more"]);
    const head = document.createElement("button");
    head.className = "agent-task-pill-head";
    const current = document.createElement("span");
    current.className = "agent-task-pill-current";
    const more = document.createElement("span");
    more.className = "agent-task-pill-more";
    head.append(current, more);
    document.body.append(head);

    expect(getComputedStyle(head).fontWeight).toBe("500");
    expect(getComputedStyle(current).fontWeight).toBe("400");
    expect(getComputedStyle(more).fontWeight).toBe("400");
    head.remove();
  });
});
