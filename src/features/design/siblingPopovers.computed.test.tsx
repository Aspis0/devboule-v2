// @vitest-environment happy-dom

// The popover shells outside the folder one, pinned on computed style read
// from the injected real sheets: every shell declares overflow-x hidden;
// overflow-wrap anywhere is declared once on the base agent picker and once on
// the history popover, and the scrolling history and skill shells declare
// overflow-y auto (the base agent picker takes its y from axis pairing).
// The base agent picker caps its height at a viewport budget and hands the
// y scroll to its options list, which is the only part allowed to shrink.
// Fixed labels inherit that wrap, and the skill-mode badge alone opts out
// with overflow-wrap normal. The craft picker and the history list are the
// real controls (the list inside a shell of the class the Design surface
// gives it); the agent shell is mounted by class — this pins declarations,
// not layout.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const mocks = vi.hoisted(() => ({
  loadDesignHistory: vi.fn(),
  sessionsList: vi.fn(),
}));

vi.mock("./designHistory", async () => {
  const actual = await vi.importActual<typeof import("./designHistory")>("./designHistory");
  return { ...actual, loadDesignHistory: mocks.loadDesignHistory };
});

vi.mock("../../lib/tauri", () => ({
  sessionsList: mocks.sessionsList,
}));

import { collectSrcSheets } from "../../styles/srcSheets";
import { assembleCssProof, removeCssProof } from "../workspace/cssProof";
import { DesignHistoryList } from "./DesignHistoryList";
import { DesignSkillModeControl } from "./DesignSkillControls";

const proof = assembleCssProof(collectSrcSheets().map((sheet) => sheet.css));

let root: ReturnType<typeof createRoot> | null = null;
const containers: HTMLElement[] = [];

afterEach(async () => {
  removeCssProof();
  if (root !== null) {
    const mounted = root;
    root = null;
    await act(async () => mounted.unmount());
  }
  for (const container of containers) container.remove();
  containers.length = 0;
});

function track(element: HTMLElement): HTMLElement {
  document.body.appendChild(element);
  containers.push(element);
  return element;
}

async function openCraftPicker(): Promise<HTMLElement> {
  const container = track(document.createElement("div"));
  root = createRoot(container);
  await act(async () => {
    root!.render(
      <DesignSkillModeControl
        skillSelection={{ version: 1, mode: "all", enabledSlugs: [] }}
        onSkillModeChange={() => undefined}
        onCraftOpen={() => undefined}
        onCraftReadMore={() => undefined}
      />,
    );
  });
  const trigger = container.querySelector<HTMLButtonElement>(
    '[data-design-skill-mode-trigger="true"]',
  );
  if (trigger === null) throw new Error("craft mode trigger did not render");
  await act(async () => trigger.click());
  const picker = container.querySelector<HTMLElement>("#design-skill-picker");
  if (picker === null) throw new Error("craft picker did not open");
  return picker;
}

async function mountHistoryList(): Promise<HTMLElement> {
  mocks.loadDesignHistory.mockResolvedValue([
    {
      sessionId: "session-1",
      peerSessionId: "peer-1",
      createdAtMs: null,
      title: "Create the final card",
      savedAtMs: 100,
      origin: "design",
    },
  ]);
  // A failing roster means "sessions unknown", which is the branch that
  // renders the unavailable line and the titles.
  mocks.sessionsList.mockRejectedValue(new Error("daemon unreachable"));
  const shell = track(document.createElement("div"));
  shell.className = "design-history-popover";
  root = createRoot(shell);
  await act(async () => {
    root!.render(<DesignHistoryList onOpen={() => undefined} />);
    await Promise.resolve();
    await Promise.resolve();
  });
  if (shell.querySelector(".design-history-unavailable") === null) {
    throw new Error("history unavailable line did not render");
  }
  if (shell.querySelector(".design-history-title") === null) {
    throw new Error("history title did not render");
  }
  return shell;
}

function mountAgentPicker(): HTMLElement {
  const shell = track(document.createElement("div"));
  shell.className = "design-agent-picker";
  return shell;
}

describe("the craft picker", () => {
  it("declares x clipped, y scrollable, text broken anywhere", async () => {
    const picker = await openCraftPicker();
    proof.inject([".design-agent-picker", ".design-skill-picker"]);
    const style = getComputedStyle(picker);
    // happy-dom pairs no axes: "visible" would read green here while a browser computes it back to auto.
    expect(["hidden", "clip"]).toContain(style.overflowX);
    expect(style.overflowY).toBe("auto");
    expect(style.overflowWrap).toBe("anywhere");
  });

  it("keeps the badge opted out and marker and action on the shell's wrapping", async () => {
    const picker = await openCraftPicker();
    proof.inject([
      ".design-agent-picker",
      ".design-skill-picker",
      ".design-skill-mode-option-badge",
      ".design-skill-mode-option-selected",
      ".design-skill-picker-action",
    ]);
    const styleOf = (selector: string): CSSStyleDeclaration => {
      const token = picker.querySelector<HTMLElement>(selector);
      if (token === null) throw new Error(`${selector} did not render`);
      return getComputedStyle(token);
    };
    const badge = styleOf(".design-skill-mode-option-badge");
    expect(badge.overflowWrap).toBe("normal");
    expect(badge.whiteSpace).not.toBe("nowrap");
    // The shell's own rule is the source of the wrapping; these two labels sit
    // under it only while their rules declare no overflow-wrap of their own.
    expect(proof.rulesFor(".design-agent-picker")).toMatch(
      /overflow-wrap:\s*(anywhere|break-word)/,
    );
    for (const selector of [".design-skill-mode-option-selected", ".design-skill-picker-action"]) {
      expect(proof.rulesFor(selector)).not.toMatch(/overflow-wrap/);
      expect(styleOf(selector).whiteSpace).not.toBe("nowrap");
    }
  });
});

describe("the history popover", () => {
  it("declares x clipped, y scrollable, text broken anywhere", async () => {
    const shell = await mountHistoryList();
    proof.inject([".design-history-popover"]);
    const style = getComputedStyle(shell);
    // happy-dom pairs no axes: "visible" would read green here while a browser computes it back to auto.
    expect(["hidden", "clip"]).toContain(style.overflowX);
    expect(style.overflowY).toBe("auto");
    expect(style.overflowWrap).toBe("anywhere");
  });

  it("keeps the history title recoverable and the time on the shell's wrapping", async () => {
    const shell = await mountHistoryList();
    const title = shell.querySelector<HTMLElement>(".design-history-title");
    if (title === null) throw new Error("history title did not render");
    const time = shell.querySelector<HTMLElement>(".design-history-list time");
    if (time === null) throw new Error("history time did not render");
    proof.inject([".design-history-popover", ".design-history-title", ".design-history-row time"]);
    const style = getComputedStyle(title);
    expect(style.textOverflow).toBe("ellipsis");
    expect(style.whiteSpace).toBe("nowrap");
    expect(style.overflow).toBe("hidden");
    expect(title.getAttribute("title")).toBe("Create the final card");
    expect(proof.rulesFor(".design-history-popover")).toMatch(
      /overflow-wrap:\s*(anywhere|break-word)/,
    );
    expect(proof.rulesFor(".design-history-row time")).not.toMatch(/overflow-wrap/);
    expect(getComputedStyle(time).whiteSpace).not.toBe("nowrap");
  });
});

describe("the agent picker shell", () => {
  it("declares x clipped and text broken anywhere", () => {
    const shell = mountAgentPicker();
    proof.inject([".design-agent-picker"]);
    const style = getComputedStyle(shell);
    // happy-dom pairs no axes: "visible" would read green here while a browser computes it back to auto.
    expect(["hidden", "clip"]).toContain(style.overflowX);
    expect(style.overflowWrap).toBe("anywhere");
  });

  it("caps its height at a viewport budget and stacks its children as a column", () => {
    proof.inject([".design-agent-picker"]);
    const rules = proof.rulesFor(".design-agent-picker");
    const cap = rules.match(/max-height:\s*([^;]+)/)?.[1] ?? "";
    // The number is the pin: 420px in a min() with the viewport terms. A
    // budget of 24px would cut the heading and every row off the menu; the
    // band token needs its fallback or the whole declaration goes invalid.
    expect(cap).toContain("420px");
    expect(cap).toContain("100vh");
    expect(cap).toContain("var(--crescent-band, 0px)");
    // The column is what lets a capped shell shrink the options list below
    // its content instead of scrolling the heading away with the list.
    expect(rules).toMatch(/display:\s*flex/);
    expect(rules).toMatch(/flex-direction:\s*column/);
  });

  it("makes its options list the scrollport: it shrinks and scrolls, the shell does not", () => {
    const shell = mountAgentPicker();
    const options = document.createElement("div");
    options.className = "design-agent-picker-options";
    shell.appendChild(options);
    const listRule = ".design-agent-picker > .design-agent-picker-options";
    proof.inject([".design-agent-picker", listRule]);
    // overflow-y: auto makes the list a scroll container, and a scroll
    // container's automatic minimum is zero — that is what lets the shell's
    // cap shrink the list and keep the heading pinned.
    expect(proof.rulesFor(listRule)).toMatch(/overflow-y:\s*auto/);
    expect(getComputedStyle(options).overflowY).toBe("auto");
    // The ring gutter, as a pair: 6px of padding holds the focused option's
    // 5px ring inside the clip, and the matching negative margin keeps every
    // edge where it was — padding alone would move all four.
    expect(getComputedStyle(options).paddingTop).toBe("6px");
    expect(getComputedStyle(options).marginTop).toBe("-6px");
  });

  it("keeps Cancel and Confirm on the shell's wrapping, never nowrap", () => {
    const classes = ["design-agent-picker-secondary", "design-agent-picker-primary"];
    const shell = mountAgentPicker();
    const tokens = classes.map((cls) => {
      const token = document.createElement("button");
      token.className = cls;
      shell.appendChild(token);
      return { cls, token };
    });
    proof.inject([".design-agent-picker", ...classes.map((cls) => `.${cls}`)]);
    expect(proof.rulesFor(".design-agent-picker")).toMatch(
      /overflow-wrap:\s*(anywhere|break-word)/,
    );
    for (const { cls, token } of tokens) {
      expect(proof.rulesFor(`.${cls}`)).not.toMatch(/overflow-wrap/);
      expect(getComputedStyle(token).whiteSpace).not.toBe("nowrap");
    }
  });
});
