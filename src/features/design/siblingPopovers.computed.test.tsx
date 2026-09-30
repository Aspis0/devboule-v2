// @vitest-environment happy-dom

// The popover shells outside the folder one, pinned on computed style read
// from the injected real sheets: each shell declares overflow-x clipped,
// overflow-y scrollable where it scrolls, and overflow-wrap anywhere once;
// fixed control labels opt out with white-space nowrap. The craft picker and
// the history list are the real controls (the list inside a shell of the class
// the Design surface gives it); the agent shell is mounted by class — this
// pins declarations, not layout.

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

  it("breaks the badge, marker and action at spaces, never mid-word", async () => {
    const picker = await openCraftPicker();
    proof.inject([
      ".design-skill-mode-option-badge",
      ".design-skill-mode-option-selected",
      ".design-skill-picker-action",
    ]);
    const tokens = [
      ".design-skill-mode-option-badge",
      ".design-skill-mode-option-selected",
      ".design-skill-picker-action",
    ].map((selector) => {
      const token = picker.querySelector<HTMLElement>(selector);
      if (token === null) throw new Error(`${selector} did not render`);
      return token;
    });
    for (const token of tokens) {
      expect(getComputedStyle(token).overflowWrap).toBe("normal");
      expect(getComputedStyle(token).whiteSpace).not.toBe("nowrap");
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

  it("keeps the history title recoverable and breaks the time at spaces only", async () => {
    const shell = await mountHistoryList();
    const title = shell.querySelector<HTMLElement>(".design-history-title");
    if (title === null) throw new Error("history title did not render");
    const time = shell.querySelector<HTMLElement>(".design-history-list time");
    if (time === null) throw new Error("history time did not render");
    proof.inject([".design-history-title", ".design-history-row time"]);
    const style = getComputedStyle(title);
    expect(style.textOverflow).toBe("ellipsis");
    expect(style.whiteSpace).toBe("nowrap");
    expect(style.overflow).toBe("hidden");
    expect(title.getAttribute("title")).toBe("Create the final card");
    expect(getComputedStyle(time).overflowWrap).toBe("normal");
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

  it("breaks Cancel and Confirm at spaces, never mid-word", () => {
    const tokens = ["design-agent-picker-secondary", "design-agent-picker-primary"].map((cls) => {
      const token = track(document.createElement("button"));
      token.className = cls;
      return token;
    });
    proof.inject([".design-agent-picker-secondary", ".design-agent-picker-primary"]);
    for (const token of tokens) {
      expect(getComputedStyle(token).overflowWrap).toBe("normal");
      expect(getComputedStyle(token).whiteSpace).not.toBe("nowrap");
    }
  });
});
