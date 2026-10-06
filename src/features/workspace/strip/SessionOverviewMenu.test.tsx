// @vitest-environment happy-dom

// The end-of-strip overview in isolation (the list) and through the strip
// (the count trigger): intent-delayed hover, press to toggle, keyboard
// travel, activation, open-tab marking, ordering, and the preview.

import { act, useRef } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MouseEvent as ReactMouseEvent } from "react";
import type { Session } from "../../../types/ipc";
import { SessionStrip } from "./SessionStrip";
import { SessionOverviewMenu } from "./SessionOverviewMenu";
import { composeStripTabs } from "./toolTabs";

function session(id: string, overrides: Partial<Session> = {}): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "acp",
    title: `title ${id}`,
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
    ...overrides,
  };
}

const ROSTER = [
  session("a", { title: "alpha", elapsedMs: 60_000 }),
  session("b", { title: "bravo", elapsedMs: 0 }),
  session("c", { title: "charlie", elapsedMs: 1_000 }),
  session("d", {
    title: "delta",
    state: {
      type: "recovered",
      generation: 1,
      integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
    },
    elapsedMs: null,
  }),
];

let root: ReturnType<typeof createRoot> | null = null;
let container: HTMLDivElement | null = null;

afterEach(() => {
  act(() => {
    root?.unmount();
  });
  container?.remove();
  root = null;
  container = null;
  vi.useRealTimers();
  vi.clearAllMocks();
});

function MenuHarness({
  sessions,
  stripOrder,
  activeSessionId,
  workspaceName,
  onOpen,
  onClose,
}: {
  sessions: readonly Session[];
  stripOrder: readonly string[];
  activeSessionId: string | null;
  workspaceName: string | null;
  onOpen: (id: string) => void;
  onClose: () => void;
}) {
  // Mirror the strip's session tabs for these session-only cases.
  const tabs = composeStripTabs(
    stripOrder.flatMap((id) => sessions.filter((session) => session.id === id)),
    [],
  );
  const triggerRef = useRef<HTMLButtonElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  return (
    <>
      <button type="button" ref={triggerRef} data-testid="trigger">
        count
      </button>
      <SessionOverviewMenu
        open
        triggerRef={triggerRef}
        contentRef={contentRef}
        sessions={sessions}
        stripOrder={stripOrder}
        tabs={tabs}
        browserPages={new Map()}
        activeTabId={activeSessionId}
        activeSessionId={activeSessionId}
        workspaceName={workspaceName}
        onOpen={onOpen}
        onSelectTab={() => {}}
        onClose={onClose}
        onListEnter={() => {}}
        onListLeave={() => {}}
      />
    </>
  );
}

function renderMenu(
  options: {
    sessions?: readonly Session[];
    stripOrder?: readonly string[];
    activeSessionId?: string | null;
    workspaceName?: string | null;
  } = {},
) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  const calls = { onOpen: vi.fn(), onClose: vi.fn() };
  act(() => {
    root!.render(
      <MenuHarness
        sessions={options.sessions ?? ROSTER}
        stripOrder={options.stripOrder ?? ["b"]}
        activeSessionId={options.activeSessionId ?? "b"}
        workspaceName={options.workspaceName ?? "atelier"}
        onOpen={calls.onOpen}
        onClose={calls.onClose}
      />,
    );
  });
  const option = (id: string): HTMLElement => {
    const element = document.querySelector<HTMLElement>(`[data-overview-option="${id}"]`);
    if (element === null) throw new Error(`overview option did not render: ${id}`);
    return element;
  };
  const optionOrder = (): string[] =>
    [...document.querySelectorAll<HTMLElement>("[data-overview-option]")].map(
      (element) => element.dataset.overviewOption ?? "",
    );
  const trigger = (): HTMLButtonElement => {
    const element = container!.querySelector<HTMLButtonElement>("[data-testid=trigger]");
    if (element === null) throw new Error("trigger did not render");
    return element;
  };
  return { ...calls, option, optionOrder, trigger };
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

function renderStrip(
  strip: Session[],
  overview: readonly Session[],
  newTabOpen = false,
  activeTabId: string | null = strip[0]?.id ?? null,
  selectedSessionId: string | null = strip[0]?.id ?? null,
  tabMenuOpen = false,
) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  const onOpenSession = vi.fn();
  const onCloseNewTabMenu = vi.fn();
  const onCloseTabMenu = vi.fn();
  const render = (sessions: readonly Session[]) => {
    act(() => {
      root!.render(
        <SessionStrip
          tabs={composeStripTabs(strip, [])}
          activeTabId={activeTabId}
          selectTab={vi.fn()}
          tabSelection={selectionStub()}
          tabClose={{
            ...noMenu,
            menu: tabMenuOpen ? { anchorId: "b", entries: [] } : null,
            closeSingle: vi.fn(),
            openMenu: vi.fn(),
            closeMenu: onCloseTabMenu,
          }}
          addButtonRef={{ current: null }}
          newTab={{
            open: newTabOpen,
            creating: false,
            workspaceSelected: true,
            onToggle: vi.fn(),
            onAgent: vi.fn(),
            onTerminal: vi.fn(),
            onBrowser: vi.fn(),
            onCloseMenu: onCloseNewTabMenu,
          }}
          providerMenu={null}
          peerNames={new Map<string, string>()}
          resolveCreator={() => null as string | null}
          takeBackAvailable={false}
          onTakeBack={vi.fn()}
          statusText={`${strip.length} sessions`}
          overviewSessions={sessions}
          workspaceName="atelier"
          onOpenSession={onOpenSession}
          selectedSessionId={selectedSessionId}
        />,
      );
    });
  };
  render(overview);
  const trigger = (): HTMLButtonElement => {
    const element = container!.querySelector<HTMLButtonElement>(".workspace-rate");
    if (element === null) throw new Error("overview trigger did not render");
    return element;
  };
  const listbox = (): HTMLElement | null => document.querySelector<HTMLElement>("[role=listbox]");
  // A roster push republishes the roster as a new array of the same rows.
  const push = () => render(overview.map((row) => ({ ...row })));
  return { onOpenSession, onCloseNewTabMenu, onCloseTabMenu, trigger, listbox, push };
}

describe("SessionOverviewMenu", () => {
  it("lists every roster session with open tabs first in strip order", () => {
    const rendered = renderMenu();
    // Only "b" is a tab; the rest follow by most recent activity.
    expect(rendered.optionOrder()).toEqual(["b", "c", "a", "d"]);
  });

  it("marks the rows that are currently open as tabs", () => {
    const rendered = renderMenu();
    expect(rendered.option("b").textContent).toContain("Open");
    expect(rendered.option("b").getAttribute("aria-label")).toContain("open tab");
    expect(rendered.option("a").textContent).not.toContain("Open");
  });

  it("names each row with its title and its state in words", () => {
    const rendered = renderMenu();
    expect(rendered.option("b").getAttribute("aria-label")).toContain("bravo");
    expect(rendered.option("b").getAttribute("aria-label")).toContain("Running");
    expect(rendered.option("d").getAttribute("aria-label")).toContain("Recovered");
  });

  it("names a row's provider the way its tab does", () => {
    const rendered = renderMenu({
      sessions: [
        session("a", { kind: "claude", provider: "claude" }),
        session("b", { provider: "gemini" }),
      ],
      stripOrder: ["a", "b"],
    });
    expect(rendered.option("a").getAttribute("aria-label")).toContain("title a, Claude, ");
    expect(rendered.option("b").getAttribute("aria-label")).toContain("title b, gemini, ");
  });

  it("activates a row on click with its session id", () => {
    const rendered = renderMenu();
    act(() => {
      rendered.option("a").dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(rendered.onOpen).toHaveBeenCalledWith("a");
  });

  it("travels with arrows, Home and End, and opens on Enter", () => {
    const rendered = renderMenu();
    act(() => {
      rendered.option("b").focus();
    });
    act(() => {
      document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
      );
    });
    expect(document.activeElement?.getAttribute("data-overview-option")).toBe("c");
    act(() => {
      document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "End", bubbles: true }),
      );
    });
    expect(document.activeElement?.getAttribute("data-overview-option")).toBe("d");
    act(() => {
      document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Home", bubbles: true }),
      );
    });
    expect(document.activeElement?.getAttribute("data-overview-option")).toBe("b");
    act(() => {
      document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      );
    });
    expect(rendered.onOpen).toHaveBeenCalledWith("b");
  });

  it("closes on Escape from inside the list", () => {
    const rendered = renderMenu();
    act(() => {
      rendered.option("c").focus();
      rendered
        .option("c")
        .dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    // The menu only closes; handing focus back is the owner's close, which
    // the strip-level test covers.
    expect(rendered.onClose).toHaveBeenCalled();
    expect(document.activeElement).toBe(rendered.option("c"));
  });

  it("previews what the roster already holds, and nothing it does not", () => {
    // Local noon pins "today" in every timezone; January is not today.
    const noon = new Date(2026, 5, 15, 12, 0, 0).getTime();
    const roster = [
      session("a", {
        title: "alpha",
        displayName: "Alpha full title",
        provider: "grok",
        elapsedMs: 300_000,
        createdAtMs: noon - 3_600_000,
        goal: "Ship the thing",
      }),
      session("b", { title: "bravo", workspaceId: null, elapsedMs: 0 }),
      session("d", {
        title: "delta",
        state: {
          type: "recovered",
          generation: 1,
          integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
        },
        elapsedMs: null,
        createdAtMs: new Date(2026, 0, 3, 9, 30, 0).getTime(),
      }),
    ];
    const rendered = renderMenu({
      sessions: roster,
      stripOrder: ["b"],
      activeSessionId: "b",
    });
    act(() => {
      rendered.option("a").dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    const preview = document.querySelector(".workspace-overview-preview")!;
    expect(preview.textContent).toContain("Alpha full title");
    expect(preview.textContent).toContain("grok");
    expect(preview.textContent).toContain("Running");
    expect(preview.textContent).toContain("atelier");
    expect(preview.textContent).toContain("5m ago");
    expect(preview.textContent).toContain("Ship the thing");
    // A recovered row has no last activity: its time cell stays empty
    // and the preview says when it started, never "active".
    expect(rendered.option("d").querySelector(".workspace-overview-time")).toBeNull();
    act(() => {
      rendered.option("d").dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    expect(preview.textContent).toContain("started");
    expect(preview.textContent).toContain("2026");
    expect(preview.textContent).not.toContain("active");
    act(() => {
      rendered.option("b").dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    expect(preview.textContent).toContain("No workspace");
  });

  it("never leaves the window: the popover carries its own max height and scrolls inside", () => {
    const height = window.innerHeight;
    Object.defineProperty(window, "innerHeight", { value: 300, configurable: true });
    try {
      renderMenu();
      const popover = document.querySelector<HTMLElement>(".workspace-overview")!;
      // AnchoredPopover writes the placement's own max height inline, from
      // the viewport — at a 300 px window the popover is capped at
      // 300 − 0 − 6 − 8, so it cannot open off-screen the way the Design
      // menu did.
      expect(popover.style.maxHeight).toBe("286px");
      expect(popover.style.overflow).toBe("auto");
      expect(document.querySelector(".workspace-overview-list")).not.toBeNull();
    } finally {
      Object.defineProperty(window, "innerHeight", { value: height, configurable: true });
    }
  });

  it("keeps ticking while open without closing or crashing", () => {
    vi.useFakeTimers();
    const rendered = renderStrip([session("b")], ROSTER);
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(rendered.listbox()).not.toBeNull();
    act(() => {
      vi.advanceTimersByTime(61_000);
    });
    expect(rendered.listbox()).not.toBeNull();
    expect(document.querySelector('[data-overview-option="b"]')).not.toBeNull();
  });
});

describe("SessionStrip overview trigger", () => {
  it("is a button named with the list's count", () => {
    const rendered = renderStrip([session("b")], ROSTER);
    const trigger = rendered.trigger();
    expect(trigger.tagName).toBe("BUTTON");
    // The visible text stays the strip's count; the name starts with it
    // and counts the list after it.
    expect(trigger.textContent).toBe("1 sessions");
    expect(trigger.getAttribute("aria-label")).toBe(
      "1 sessions — show all 1 tab and 3 more sessions",
    );
  });

  it("pins a hover-opened list on click instead of dismissing it", () => {
    vi.useFakeTimers();
    const rendered = renderStrip([session("b")], ROSTER);
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(200);
    });
    expect(rendered.listbox()).not.toBeNull();
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(rendered.listbox()).not.toBeNull();
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(rendered.listbox()).toBeNull();
  });

  it("yields a standing sibling menu when the overview opens", () => {
    vi.useFakeTimers();
    const rendered = renderStrip([session("b")], ROSTER, true);
    expect(document.querySelector('[aria-label="New tab"]')).not.toBeNull();
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(200);
    });
    expect(rendered.listbox()).not.toBeNull();
    expect(rendered.onCloseNewTabMenu).toHaveBeenCalled();
  });

  it("yields a standing tab menu when the overview opens", () => {
    vi.useFakeTimers();
    const rendered = renderStrip([session("b")], ROSTER, false, "b", "b", true);
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(200);
    });
    expect(rendered.listbox()).not.toBeNull();
    expect(rendered.onCloseTabMenu).toHaveBeenCalled();
  });

  it("closes on Escape however the list was opened", () => {
    vi.useFakeTimers();
    const rendered = renderStrip([session("b")], ROSTER);
    const pane = document.createElement("div");
    pane.tabIndex = -1;
    container!.appendChild(pane);
    // Hover-open leaves focus outside the portal: the list's own Escape
    // never fires there, so the document listener must close it.
    act(() => {
      pane.focus();
    });
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(200);
    });
    expect(rendered.listbox()).not.toBeNull();
    act(() => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(rendered.listbox()).toBeNull();
    // Same for an explicitly opened list with focus moved away.
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(rendered.listbox()).not.toBeNull();
    act(() => {
      pane.focus();
    });
    act(() => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(rendered.listbox()).toBeNull();
  });

  it("keeps the hovered preview and roving focus across roster pushes", () => {
    const rendered = renderStrip([session("b")], ROSTER);
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const row = document.querySelector<HTMLElement>('[data-overview-option="a"]')!;
    act(() => {
      row.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
      row.focus();
    });
    // A push republishes the roster as a new array of the same rows: the
    // preview and the roving focus must stay on the hovered row, so the
    // next ArrowDown continues from there instead of the active tab.
    act(() => {
      rendered.push();
    });
    expect(document.querySelector(".workspace-overview-preview")?.textContent).toContain("alpha");
    expect(document.querySelector<HTMLElement>('[data-overview-option="a"]')?.tabIndex).toBe(0);
    act(() => {
      document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
      );
    });
    expect(document.activeElement?.getAttribute("data-overview-option")).toBe("d");
  });

  it("opens on click and closes on the next one", () => {
    const rendered = renderStrip([session("b")], ROSTER);
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(rendered.listbox()).not.toBeNull();
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(rendered.listbox()).toBeNull();
  });

  it("renders the empty sentence beside the listbox, not inside it", () => {
    const rendered = renderMenu({ sessions: [], stripOrder: [], activeSessionId: null });
    // A listbox owns only options: with no rows there is no listbox, and
    // the sentence stands beside where it would be.
    expect(document.querySelector('[role="listbox"]')).toBeNull();
    expect(document.querySelector(".workspace-overview-empty")?.textContent).toContain(
      "No tabs in this workspace.",
    );
    expect(rendered.optionOrder()).toEqual([]);
  });

  it("falls back to the first row when the active id names no row", () => {
    // The seed and the roving fallback share one rule: a stale id never
    // strands DOM focus while tabIndex=0 sits on another row.
    const rendered = renderStrip([session("b")], ROSTER, false, "b", "gone");
    act(() => {
      rendered.trigger().focus();
    });
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(rendered.listbox()).not.toBeNull();
    expect(document.activeElement?.getAttribute("data-overview-option")).toBe("b");
  });

  it("seeds the list from the selected session when a tool tab is active", () => {
    // The tab id names a tool tab, which matches no row: the menu must
    // still move focus in, starting from the selected session's row.
    const rendered = renderStrip([session("b")], ROSTER, false, "tool:diff:ws:a.ts", "b");
    act(() => {
      rendered.trigger().focus();
    });
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(rendered.listbox()).not.toBeNull();
    expect(document.activeElement?.getAttribute("data-overview-option")).toBe("b");
  });

  it("moves focus into the list when the trigger holds it", () => {
    const rendered = renderStrip([session("b")], ROSTER);
    act(() => {
      rendered.trigger().focus();
    });
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    // The keyboard-open path: focus starts on the active session's row.
    expect(document.activeElement?.getAttribute("data-overview-option")).toBe("b");
  });

  it("never steals focus on hover", () => {
    vi.useFakeTimers();
    const rendered = renderStrip([session("b")], ROSTER);
    const pane = document.createElement("div");
    pane.tabIndex = -1;
    container!.appendChild(pane);
    act(() => {
      pane.focus();
    });
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(200);
    });
    expect(rendered.listbox()).not.toBeNull();
    expect(document.activeElement).toBe(pane);
  });

  it("opens on hover only after the intent delay, and survives the crossing into the list", () => {
    vi.useFakeTimers();
    const rendered = renderStrip([session("b")], ROSTER);
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(149);
    });
    expect(rendered.listbox()).toBeNull();
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(rendered.listbox()).not.toBeNull();
    // Leaving the trigger starts the grace delay, not the close.
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("mouseout", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(100);
    });
    expect(rendered.listbox()).not.toBeNull();
    // Reaching the list cancels it.
    const popover = document.querySelector<HTMLElement>(".workspace-overview")!;
    act(() => {
      popover.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(500);
    });
    expect(rendered.listbox()).not.toBeNull();
    // Leaving both closes after the same delay.
    act(() => {
      popover.dispatchEvent(new MouseEvent("mouseout", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(149);
    });
    expect(rendered.listbox()).not.toBeNull();
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(rendered.listbox()).toBeNull();
  });

  it("hands focus back to the trigger when the pointer leaves a keyboard-opened list", () => {
    vi.useFakeTimers();
    const rendered = renderStrip([session("b")], ROSTER);
    act(() => {
      rendered.trigger().focus();
    });
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    // Press-open moved DOM focus onto the row; the pointer never entered.
    const row = document.querySelector<HTMLElement>('[data-overview-option="b"]')!;
    expect(document.activeElement).toBe(row);
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("mouseout", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(150);
    });
    // The hover-leave close must run the guarded close, not drop focus.
    expect(rendered.listbox()).toBeNull();
    expect(document.activeElement).toBe(rendered.trigger());
  });

  it("leaves focus on the row when the pointer re-enters during the grace", () => {
    vi.useFakeTimers();
    const rendered = renderStrip([session("b")], ROSTER);
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(200);
    });
    expect(rendered.listbox()).not.toBeNull();
    // A keyboard user reading a row while the pointer drifts back: the
    // re-entry continues the open menu instead of dismissing it against
    // itself and yanking focus to the trigger.
    const row = document.querySelector<HTMLElement>('[data-overview-option="b"]')!;
    act(() => {
      row.focus();
    });
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("mouseout", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(100);
    });
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    act(() => {
      vi.advanceTimersByTime(200);
    });
    expect(rendered.listbox()).not.toBeNull();
    expect(document.activeElement).toBe(row);
  });

  it("closes on Escape with focus back on the trigger, and opens the picked session", () => {
    const strip = [session("b")];
    const rendered = renderStrip(strip, ROSTER);
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const row = document.querySelector<HTMLElement>('[data-overview-option="a"]')!;
    act(() => {
      row.focus();
      row.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(rendered.listbox()).toBeNull();
    expect(document.activeElement).toBe(rendered.trigger());
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const reopened = document.querySelector<HTMLElement>('[data-overview-option="a"]')!;
    act(() => {
      reopened.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(rendered.onOpenSession).toHaveBeenCalledWith("a");
    expect(rendered.listbox()).toBeNull();
  });
});
