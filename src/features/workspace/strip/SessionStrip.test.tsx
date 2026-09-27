// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MouseEvent as ReactMouseEvent } from "react";
import type { Session } from "../../../types/ipc";
import { SessionStrip } from "./SessionStrip";

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
  activateEntry: vi.fn(),
  confirmClose: vi.fn(),
  cancelClose: vi.fn(),
};

function selectionStub() {
  return {
    selection: new Set<string>(),
    announcement: "",
    handleTabClick: vi.fn((_session: Session, _event: ReactMouseEvent<HTMLButtonElement>) => {}),
    clearSelection: vi.fn(),
  };
}

function propsOf(sessions: Session[], selectedSessionId: string | null) {
  return {
    sessions,
    selectedSessionId,
    selectSession: vi.fn(),
    tabSelection: selectionStub(),
    tabClose: { ...noMenu, closeSingle: vi.fn(), openMenu: vi.fn() },
    addButtonRef: { current: null },
    newTab: {
      open: false,
      creating: false,
      workspaceSelected: true,
      onToggle: vi.fn(),
      onAgent: vi.fn(),
      onTerminal: vi.fn(),
      onCloseMenu: vi.fn(),
    },
    providerMenu: null,
    peerNames: new Map<string, string>(),
    resolveCreator: () => null as string | null,
    takeBackAvailable: false,
    onTakeBack: vi.fn(),
    statusText: `${sessions.length} sessions`,
  };
}

let root: ReturnType<typeof createRoot> | null = null;
let container: HTMLDivElement | null = null;

function renderStrip(sessions: Session[], selected: string | null) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  const props = propsOf(sessions, selected);
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
  it("renders one tab per session with a dot, a kind mark and a clipped label", () => {
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
      expect(tab.querySelector(".workspace-tab-meta")).toBeNull();
      expect(tab.querySelector(".workspace-tab-attention")).toBeNull();
      expect(tab.querySelector(".workspace-tab-delegation")).toBeNull();
      expect(tab.querySelector(".workspace-session-origin-badge")).toBeNull();
    }
  });

  it("keeps Needs your approval as the one chip that still uses words", () => {
    renderStrip([session("a", "agent a", { attention: { reason: "permission", atMs: 1 } })], "a");
    const tab = container!.querySelector<HTMLElement>(".workspace-session-tab")!;
    expect(tab.querySelector(".workspace-tab-attention")?.textContent).toBe("Needs your approval");
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
    expect(props.selectSession).toHaveBeenCalledWith("b");
    expect(document.activeElement?.getAttribute("id")).toContain("b");
    act(() => {
      (document.activeElement as HTMLElement).dispatchEvent(
        new KeyboardEvent("keydown", { key: "End", bubbles: true }),
      );
    });
    expect(props.selectSession).toHaveBeenCalledWith("c");
    act(() => {
      (document.activeElement as HTMLElement).dispatchEvent(
        new KeyboardEvent("keydown", { key: "Home", bubbles: true }),
      );
    });
    expect(props.selectSession).toHaveBeenCalledWith("a");
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
    expect(props.tabClose.closeSingle).toHaveBeenCalledWith("b");
  });

  it("switches tabs on Alt+Shift+] and Alt+Shift+[", () => {
    const props = renderStrip([session("a", "agent a"), session("b", "agent b")], "a");
    act(() => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "]", altKey: true, shiftKey: true, bubbles: true }),
      );
    });
    expect(props.selectSession).toHaveBeenCalledWith("b");
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
    expect(props.selectSession).not.toHaveBeenCalled();
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
    expect(props.selectSession).not.toHaveBeenCalled();
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
    expect(props.selectSession).not.toHaveBeenCalled();
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
    expect(props.selectSession).not.toHaveBeenCalled();
  });

  it("paints every state with its own dot tone", () => {
    renderStrip(
      [
        session("live", "live one"),
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
    expect(tones[5].contains("strip-dot-attention")).toBe(true);
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
    expect(tab.querySelector(".workspace-tab-meta")).toBeNull();
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

  it("announces the selection size politely", () => {
    renderStrip([session("a", "agent a")], "a");
    expect(container!.querySelector('[role="status"]')).not.toBeNull();
  });
});
