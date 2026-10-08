// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

declare global {
  // eslint-disable-next-line no-var
  var __stripChipRenders: number | undefined;
}

vi.mock("./StripChip", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./StripChip")>();
  const Counted = (props: Parameters<typeof actual.StripChip>[0]) => {
    globalThis.__stripChipRenders = (globalThis.__stripChipRenders ?? 0) + 1;
    return actual.StripChip(props);
  };
  return { ...actual, StripChip: Counted };
});
import type { MouseEvent as ReactMouseEvent } from "react";
import type { Session } from "../../../types/ipc";
import { SessionStrip } from "./SessionStrip";
import { composeStripTabs } from "./toolTabs";

function session(id: string, title: string, overrides: Partial<Session> = {}): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "acp",
    title,
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
    ...overrides,
  };
}

const noMenu = {
  menu: null,
  anchorRef: { current: null },
  confirm: null,
  openMenu: vi.fn(),
  closeMenu: vi.fn(),
  closeSingle: vi.fn(),
  closeTab: vi.fn(),
  activateEntry: vi.fn(),
  copyEntryValue: vi.fn(() => null),
  activatePaneEntry: vi.fn(),
  confirmClose: vi.fn(),
  cancelClose: vi.fn(),
};

function selectionStub() {
  return {
    selection: new Set<string>(),
    announcement: "",
    handleTabClick: vi.fn((_tab: { id: string }, _event: ReactMouseEvent<HTMLButtonElement>) => {}),
    clearSelection: vi.fn(),
  };
}

function propsOf(sessions: Session[], activeTabId: string | null, overview = sessions) {
  return {
    tabs: composeStripTabs(sessions, []),
    activeTabId,
    selectTab: vi.fn(),
    tabSelection: selectionStub(),
    tabClose: { ...noMenu, closeTab: vi.fn(), openMenu: vi.fn() },
    addButtonRef: { current: null },
    newTab: {
      open: false,
      creating: false,
      workspaceSelected: true,
      onToggle: vi.fn(),
      onAgent: vi.fn(),
      onTerminal: vi.fn(),
      onBrowser: vi.fn(),
      onCloseMenu: vi.fn(),
    },
    providerMenu: null,
    peerNames: new Map<string, string>(),
    resolveCreator: () => null as string | null,
    takeBackAvailable: false,
    onTakeBack: vi.fn(),
    overviewSessions: overview,
    workspaceName: "workspace one",
    onOpenSession: vi.fn(),
    selectedSessionId: sessions[0]?.id ?? null,
  };
}

let root: ReturnType<typeof createRoot> | null = null;
let container: HTMLDivElement | null = null;

function renderStrip(sessions: Session[], selected: string | null, overview = sessions) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  const props = propsOf(sessions, selected, overview);
  act(() => {
    root!.render(<SessionStrip {...props} />);
  });
  return {
    ...props,
    rerender: (nextSessions: Session[], nextSelected: string | null) => {
      const next = propsOf(nextSessions, nextSelected);
      act(() => {
        root!.render(<SessionStrip {...next} />);
      });
      return next;
    },
  };
}

afterEach(() => {
  act(() => {
    root?.unmount();
  });
  container?.remove();
  root = null;
  container = null;
  vi.clearAllMocks();
});

describe("SessionStrip", () => {
  it("renders one tab per explicitly supplied open session with a dot, a kind mark and a clipped label", () => {
    renderStrip([session("a", "agent a"), session("b", "agent b")], "a");
    const tabs = [...container!.querySelectorAll<HTMLElement>(".workspace-session-tab")];
    expect(tabs).toHaveLength(2);
    for (const tab of tabs) {
      expect(tab.getAttribute("role")).toBe("tab");
      expect(tab.querySelector(".workspace-status-dot")).not.toBeNull();
      expect(tab.querySelector(".strip-kind")).not.toBeNull();
      expect(tab.querySelector(".workspace-tab-label")?.textContent).toMatch(/agent/);
    }
    expect(tabs[0].getAttribute("aria-selected")).toBe("true");
  });

  it("carries no state words, badges or pills on the chip", () => {
    renderStrip(
      [
        session("a", "agent a", { attention: { reason: "finished", atMs: 1 } }),
        session("b", "agent b", {
          createdBy: "someone",
          origin: { kind: "peer", deviceId: "d1" },
          unattended: "yes",
        }),
      ],
      "a",
    );
    const tabs = [...container!.querySelectorAll<HTMLElement>(".workspace-session-tab")];
    for (const tab of tabs) {
      expect(tab.querySelector(".workspace-tab-attention")).toBeNull();
      expect(tab.querySelector(".workspace-tab-delegation")).toBeNull();
      expect(tab.querySelector(".workspace-session-origin-badge")).toBeNull();
    }
  });

  it("paints a tab with attention in label only: the state is heard, never seen", () => {
    // The spec's chip is dot + kind mark + label + hover × — a state is a
    // dot tone, never a sentence. The permission ask is the ochre dot; its
    // words live in the accessible name and the description beside the tab.
    renderStrip([session("a", "agent a", { attention: { reason: "permission", atMs: 1 } })], "a");
    const tab = container!.querySelector<HTMLElement>(".workspace-session-tab")!;
    expect(tab.querySelector(".workspace-tab-attention")).toBeNull();
    const label = tab.querySelector(".workspace-tab-label");
    expect(label?.textContent).toBe("agent a");
    // The button's children are exactly the chip's four parts — dot, mark,
    // label, and the sr-only state the chip's CSS hides. A fifth child
    // (a pill, a badge) would paint words the spec does not allow.
    expect([...tab.children].map((child) => child.className)).toEqual([
      expect.stringContaining("workspace-status-dot"),
      "strip-kind",
      "workspace-tab-label",
      "workspace-sr-only",
    ]);
    const state = tab.querySelector(".workspace-sr-only");
    expect(state?.textContent).toBe("Agent, Running");
    // And the ask itself describes the chip, from beside the button.
    const describedBy = tab.getAttribute("aria-describedby");
    expect(describedBy).not.toBeNull();
    const provenance = container!.querySelector<HTMLElement>(`#${CSS.escape(describedBy!)}`);
    expect(provenance?.textContent).toContain("Needs your approval");
  });

  it("drops the ask's description when the attention clears", () => {
    // The description span lives only while there is something to
    // describe: a roster push that clears the attention must take the
    // span and the reference with it, or a screen reader keeps
    // announcing a resolved ask. A local session with no ask and no
    // creator has nothing to describe — an originless one always carries
    // the "origin unknown" line, so the local origin is the empty case.
    const local = { origin: { kind: "local" as const } };
    const props = renderStrip(
      [session("a", "agent a", { ...local, attention: { reason: "permission", atMs: 1 } })],
      "a",
    );
    const tab = container!.querySelector<HTMLElement>(".workspace-session-tab")!;
    expect(tab.getAttribute("aria-describedby")).not.toBeNull();
    props.rerender([session("a", "agent a", local)], "a");
    expect(tab.getAttribute("aria-describedby")).toBeNull();
    // The state line stays: only the provenance span is gone.
    expect(container!.querySelector("#workspace-session-tab-a-provenance")).toBeNull();
    expect(tab.querySelector(".workspace-sr-only")?.textContent).toBe("Agent, Running");
  });

  it("moves origin and creator into the tooltip", () => {
    const props = propsOf(
      [session("a", "agent a", { origin: { kind: "peer", deviceId: "d1" } })],
      "a",
    );
    props.peerNames = new Map([["d1", "pixel"]]);
    props.resolveCreator = () => "created by planner";
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    act(() => {
      root!.render(<SessionStrip {...props} />);
    });
    const tab = container!.querySelector<HTMLElement>(".workspace-session-tab")!;
    expect(tab.getAttribute("title")).toContain("from pixel");
    expect(tab.getAttribute("title")).toContain("created by planner");
  });

  it("draws no session count: the overview trigger is a glyph, its count lives in the label", () => {
    renderStrip([session("a", "agent a"), session("b", "agent b")], "a");
    const trigger = container!.querySelector<HTMLButtonElement>(".workspace-rate")!;
    expect(trigger.getAttribute("aria-label")).toBe("Show all sessions — 2 open");
    expect(trigger.textContent).toBe("");
    expect(container!.textContent).not.toMatch(/open session|d+ sessions?/);
  });

  it("names the sessions waiting on approval in the trigger label", () => {
    const open = session("a", "agent a");
    const waiting = [
      session("b", "agent b", { attention: { reason: "permission", atMs: 0 } }),
      session("c", "agent c", { attention: { reason: "permission", atMs: 0 } }),
    ];
    renderStrip([open], "a", [open, ...waiting]);
    const trigger = container!.querySelector<HTMLButtonElement>(".workspace-rate")!;
    expect(trigger.getAttribute("aria-label")).toBe("Show all sessions — 1 open, 2 need approval");
  });

  it("singularises the approval count", () => {
    const open = session("a", "agent a");
    const waiting = session("b", "agent b", { attention: { reason: "permission", atMs: 0 } });
    renderStrip([open], "a", [open, waiting]);
    const trigger = container!.querySelector<HTMLButtonElement>(".workspace-rate")!;
    expect(trigger.getAttribute("aria-label")).toBe("Show all sessions — 1 open, 1 needs approval");
  });

  it("leaves approval out of the label when the waiting session already has a tab", () => {
    const waiting = session("a", "agent a", { attention: { reason: "permission", atMs: 0 } });
    renderStrip([waiting], "a");
    const trigger = container!.querySelector<HTMLButtonElement>(".workspace-rate")!;
    expect(trigger.getAttribute("aria-label")).toBe("Show all sessions — 1 open");
  });

  it("keeps the add button outside the box that scrolls", () => {
    renderStrip([session("a", "agent a")], "a");
    const scroller = container!.querySelector<HTMLElement>(".workspace-session-tabs-scroll")!;
    const add = container!.querySelector<HTMLElement>(".workspace-session-add")!;
    expect(scroller.contains(add)).toBe(false);
    expect(scroller.classList.contains("workspace-scroll")).toBe(false);
  });

  it("shows the fade only on the side that still hides chips", async () => {
    renderStrip([session("a", "agent a"), session("b", "agent b")], "a");
    const scroller = container!.querySelector<HTMLElement>(".workspace-session-tabs-scroll")!;
    Object.defineProperties(scroller, {
      scrollWidth: { value: 500, configurable: true },
      clientWidth: { value: 200, configurable: true },
      scrollLeft: { value: 0, writable: true, configurable: true },
    });
    act(() => {
      scroller.dispatchEvent(new Event("scroll", { bubbles: true }));
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    expect(scroller.dataset.fadeRight).toBe("true");
    expect(scroller.dataset.fadeLeft).toBe("false");
    act(() => {
      Object.defineProperty(scroller, "scrollLeft", { value: 300, writable: true });
      scroller.dispatchEvent(new Event("scroll", { bubbles: true }));
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    expect(scroller.dataset.fadeRight).toBe("false");
    expect(scroller.dataset.fadeLeft).toBe("true");
  });

  it("hides the fade when every chip fits", async () => {
    renderStrip([session("a", "agent a")], "a");
    const scroller = container!.querySelector<HTMLElement>(".workspace-session-tabs-scroll")!;
    Object.defineProperties(scroller, {
      scrollWidth: { value: 150, configurable: true },
      clientWidth: { value: 200, configurable: true },
      scrollLeft: { value: 0, writable: true, configurable: true },
    });
    act(() => {
      scroller.dispatchEvent(new Event("scroll", { bubbles: true }));
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    expect(scroller.dataset.fadeRight).toBe("false");
    expect(scroller.dataset.fadeLeft).toBe("false");
  });

  it("moves between chips with arrows and Home/End, selecting as it goes", () => {
    const props = renderStrip(
      [session("a", "agent a"), session("b", "agent b"), session("c", "agent c")],
      "a",
    );
    const tabs = () => [
      ...container!.querySelectorAll<HTMLButtonElement>(".workspace-session-tab"),
    ];
    expect(tabs()[0].tabIndex).toBe(0);
    expect(tabs()[1].tabIndex).toBe(-1);
    act(() => {
      tabs()[0].focus();
      tabs()[0].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    });
    expect(props.selectTab).toHaveBeenCalledWith("b");
    expect(document.activeElement?.getAttribute("id")).toContain("b");
    act(() => {
      (document.activeElement as HTMLElement).dispatchEvent(
        new KeyboardEvent("keydown", { key: "End", bubbles: true }),
      );
    });
    expect(props.selectTab).toHaveBeenCalledWith("c");
    act(() => {
      (document.activeElement as HTMLElement).dispatchEvent(
        new KeyboardEvent("keydown", { key: "Home", bubbles: true }),
      );
    });
    expect(props.selectTab).toHaveBeenCalledWith("a");
  });

  it("closes the focused chip with Delete without tabbing through a close button", () => {
    const props = renderStrip([session("a", "agent a"), session("b", "agent b")], "a");
    const tabs = [...container!.querySelectorAll<HTMLButtonElement>(".workspace-session-tab")];
    const closes = [
      ...container!.querySelectorAll<HTMLButtonElement>(".workspace-session-chip-close"),
    ];
    for (const close of closes) expect(close.tabIndex).toBe(-1);
    act(() => {
      tabs[1].focus();
      tabs[1].dispatchEvent(new KeyboardEvent("keydown", { key: "Delete", bubbles: true }));
    });
    expect(props.tabClose.closeTab).toHaveBeenCalledWith("b");
  });

  it("switches tabs on Alt+Shift+] and Alt+Shift+[", () => {
    const props = renderStrip([session("a", "agent a"), session("b", "agent b")], "a");
    act(() => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "]", altKey: true, shiftKey: true, bubbles: true }),
      );
    });
    expect(props.selectTab).toHaveBeenCalledWith("b");
  });

  it("does not steal Alt+Shift+] from the terminal", () => {
    const props = renderStrip([session("a", "agent a"), session("b", "agent b")], "a");
    const shell = document.createElement("div");
    shell.className = "workspace-terminal-shell";
    shell.tabIndex = -1;
    container!.appendChild(shell);
    act(() => {
      shell.dispatchEvent(
        new KeyboardEvent("keydown", { key: "]", altKey: true, shiftKey: true, bubbles: true }),
      );
    });
    expect(props.selectTab).not.toHaveBeenCalled();
    shell.remove();
  });

  it.each([["input"], ["textarea"], ["select"]])("does not switch tabs from a %s", (tag) => {
    const props = renderStrip([session("a", "agent a"), session("b", "agent b")], "a");
    const field = document.createElement(tag);
    container!.appendChild(field);
    act(() => {
      field.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "]",
          altKey: true,
          shiftKey: true,
          bubbles: true,
        }),
      );
    });
    expect(props.selectTab).not.toHaveBeenCalled();
  });

  it("does not switch tabs from an editable region", () => {
    const props = renderStrip([session("a", "agent a"), session("b", "agent b")], "a");
    const editable = document.createElement("div");
    editable.contentEditable = "true";
    container!.appendChild(editable);
    act(() => {
      editable.dispatchEvent(
        new KeyboardEvent("keydown", { key: "]", altKey: true, shiftKey: true, bubbles: true }),
      );
    });
    expect(props.selectTab).not.toHaveBeenCalled();
  });

  it("does not switch tabs while the keys belong to a composition", () => {
    const props = renderStrip([session("a", "agent a"), session("b", "agent b")], "a");
    const field = document.createElement("textarea");
    container!.appendChild(field);
    const event = new KeyboardEvent("keydown", {
      key: "]",
      altKey: true,
      shiftKey: true,
      bubbles: true,
    });
    Object.defineProperty(event, "isComposing", { value: true });
    act(() => {
      field.dispatchEvent(event);
    });
    expect(props.selectTab).not.toHaveBeenCalled();
  });

  it("does not switch tabs on a composing chord outside any field", () => {
    // The tag guard alone would let this through: only the isComposing
    // guard stands between a composing chord on the focused chip and a tab
    // switch that yanks focus away mid-composition.
    const props = renderStrip([session("a", "agent a"), session("b", "agent b")], "a");
    const tab = container!.querySelector<HTMLElement>(".workspace-session-tab")!;
    const event = new KeyboardEvent("keydown", {
      key: "]",
      altKey: true,
      shiftKey: true,
      bubbles: true,
    });
    Object.defineProperty(event, "isComposing", { value: true });
    act(() => {
      tab.dispatchEvent(event);
    });
    expect(props.selectTab).not.toHaveBeenCalled();
  });

  it("does not switch tabs on a keyCode-229 chord from a legacy engine", () => {
    // One definition serves every site: legacy engines report the
    // composition as keyCode 229 without ever setting isComposing.
    const props = renderStrip([session("a", "agent a"), session("b", "agent b")], "a");
    const tab = container!.querySelector<HTMLElement>(".workspace-session-tab")!;
    const event = new KeyboardEvent("keydown", {
      key: "]",
      altKey: true,
      shiftKey: true,
      bubbles: true,
      keyCode: 229,
    });
    act(() => {
      tab.dispatchEvent(event);
    });
    expect(props.selectTab).not.toHaveBeenCalled();
  });

  it("paints every state with its own dot tone", () => {
    renderStrip(
      [
        session("live", "live one", { activity: "working" }),
        session("silent", "quiet one", {
          state: { type: "silent", generation: 1 },
          elapsedMs: 60_000,
        }),
        session("ended", "stopped one", {
          state: { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } },
        }),
        session("recovered", "back one", {
          state: {
            type: "recovered",
            generation: 1,
            integrity: {
              kind: "unverifiable",
              droppedFrames: 0,
              droppedBytes: 0,
              trimmedBytes: 0,
            },
          },
        }),
        session("mystery", "unknown one", {
          state: { type: "nope" } as unknown as Session["state"],
        }),
        session("attention", "hot one", { attention: { reason: "error", atMs: 1 } }),
        session("unattended", "free one", { unattended: "yes" }),
      ],
      "live",
    );
    const dots = [...container!.querySelectorAll(".workspace-status-dot")];
    expect(dots).toHaveLength(7);
    const tones = dots.map((dot) => dot.classList);
    expect(tones[0].contains("strip-dot-live")).toBe(true);
    expect(tones[0].contains("dot-pulse")).toBe(true);
    expect(tones[1].contains("strip-dot-idle")).toBe(true);
    expect(tones[2].contains("strip-dot-ended")).toBe(true);
    expect(tones[3].contains("strip-dot-recovered")).toBe(true);
    expect(tones[4].contains("strip-dot-unknown")).toBe(true);
    expect(tones[5].contains("strip-dot-failed")).toBe(true);
    expect(tones[5].contains("strip-dot-attention")).toBe(false);
    expect(tones[6].contains("strip-dot-unattended")).toBe(true);
  });

  it("exposes the state words to assistive tech without painting them", () => {
    renderStrip(
      [
        session("s", "quiet one", {
          state: { type: "silent", generation: 1 },
          elapsedMs: 60_000,
        }),
      ],
      "s",
    );
    const tab = container!.querySelector<HTMLElement>(".workspace-session-tab")!;
    expect(tab.querySelector(".workspace-sr-only")?.textContent).toContain("Quiet");
    expect(tab.getAttribute("aria-keyshortcuts")).toContain("Delete");
  });

  it("keeps one tab stop while the selection still points outside the strip", () => {
    // Workspace reconciles the selection in an effect after the commit that
    // drops the selected row; until then the strip must stay reachable.
    const first = renderStrip([session("a", "agent a"), session("b", "agent b")], "b");
    first.rerender([session("a", "agent a")], "b");
    const tabs = [...container!.querySelectorAll<HTMLButtonElement>(".workspace-session-tab")];
    expect(tabs).toHaveLength(1);
    expect(tabs[0].tabIndex).toBe(0);
  });

  it("describes provenance to assistive tech without painting it", () => {
    const props = propsOf(
      [
        session("a", "agent a", {
          origin: { kind: "peer", deviceId: "d1" },
          unattended: "yes",
        }),
      ],
      "a",
    );
    props.peerNames = new Map([["d1", "pixel"]]);
    props.resolveCreator = () => "created by planner";
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    act(() => {
      root!.render(<SessionStrip {...props} />);
    });
    const tab = container!.querySelector<HTMLElement>(".workspace-session-tab")!;
    // One description source: the provenance lines live in the
    // described-by span, not only in the title a reader may never hear.
    const describedBy = tab.getAttribute("aria-describedby");
    expect(describedBy).not.toBeNull();
    const provenance = container!.querySelector<HTMLElement>(`#${CSS.escape(describedBy!)}`);
    expect(provenance?.textContent).toContain("from pixel");
    expect(provenance?.textContent).toContain("created by planner");
    expect(provenance?.textContent).toContain("auto-accepting");
    // The state still names the chip exactly once.
    expect(tab.querySelector(".workspace-sr-only")?.textContent).toContain("Running");
  });

  it("leaves a chip with no provenance undescribed", () => {
    // Local, human-started, live: no state details, no origin line, no
    // creator — nothing for a description to carry. (An absent origin
    // would be provenance: "origin unknown".)
    renderStrip([session("a", "agent a", { origin: { kind: "local" } })], "a");
    const tab = container!.querySelector<HTMLElement>(".workspace-session-tab")!;
    expect(tab.getAttribute("aria-describedby")).toBeNull();
  });

  it("does not re-render chips when the parent re-renders around them", () => {
    // Unrelated Workspace renders (composer keystrokes, pane state) must
    // not walk the chips: same inputs in, zero chip renders out. Without
    // the rows memo every chip function runs again and the count climbs.
    const sessions = [session("a", "agent a"), session("b", "agent b")];
    const stable = propsOf(sessions, "a");
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    act(() => {
      root!.render(<SessionStrip {...stable} />);
    });
    globalThis.__stripChipRenders = 0;
    act(() => {
      root!.render(<SessionStrip {...stable} />);
    });
    expect(globalThis.__stripChipRenders).toBe(0);
  });

  it("names the chip with label and state only, never the provenance paragraph", () => {
    // happy-dom has no accname engine; the button's text content is the
    // exact set accname collects from, so equality here is the name.
    const props = propsOf(
      [
        session("a", "agent a", {
          origin: { kind: "peer", deviceId: "d1" },
          unattended: "yes",
        }),
      ],
      "a",
    );
    props.peerNames = new Map([["d1", "pixel"]]);
    props.resolveCreator = () => "created by planner";
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    act(() => {
      root!.render(<SessionStrip {...props} />);
    });
    const tab = container!.querySelector<HTMLElement>(".workspace-session-tab")!;
    const label = tab.querySelector(".workspace-tab-label")?.textContent ?? "";
    const state = tab.querySelector(".workspace-sr-only")?.textContent ?? "";
    expect(label).toBe("agent a");
    expect(tab.textContent).toBe(label + state);
    // The provenance still describes the chip, from beside the button.
    const describedBy = tab.getAttribute("aria-describedby")!;
    expect(tab.querySelector(`#${describedBy}`)).toBeNull();
    const row = tab.closest(".workspace-session-row")!;
    expect(row.querySelector(`#${describedBy}`)?.textContent).toContain("created by planner");
  });

  it("marks a tab that is both selected and multi-selected", () => {
    // The state the hover cascade resolves: both classes on one chip.
    const props = propsOf([session("a", "agent a"), session("b", "agent b")], "a");
    props.tabSelection.selection.add("a");
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    act(() => {
      root!.render(<SessionStrip {...props} />);
    });
    const tab = container!.querySelector<HTMLElement>("#workspace-session-tab-a")!;
    expect(tab.classList.contains("workspace-session-tab-selected")).toBe(true);
    expect(tab.classList.contains("workspace-session-tab-multiselected")).toBe(true);
  });

  it("announces the selection size politely", () => {
    renderStrip([session("a", "agent a")], "a");
    expect(container!.querySelector('[role="status"]')).not.toBeNull();
  });
});
