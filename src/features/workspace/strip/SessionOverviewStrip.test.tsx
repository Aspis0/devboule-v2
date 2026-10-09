// @vitest-environment happy-dom

// The overview reached through the strip: the trigger's accessible count,
// a tool row's selection round-trip, and Escape handing focus back.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

import type { MouseEvent as ReactMouseEvent } from "react";
import type { Session } from "../../../types/ipc";
import { SessionStrip } from "./SessionStrip";
import { composeStripTabs, makeToolTab, type StripTab } from "./toolTabs";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";

const keyFor = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

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

const FILE = makeToolTab("file", keyFor("workspace-1"), "notes/todo.md");
const DIFF = makeToolTab("diff", keyFor("workspace-1"), "src/app.ts");

const ROSTER = [
  session("a", { title: "alpha", elapsedMs: 60_000 }),
  session("b", { title: "bravo", elapsedMs: 0 }),
  session("c", { title: "charlie", elapsedMs: 1_000 }),
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

function renderStrip(tabs: StripTab[], overview: readonly Session[], activeTabId: string | null) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  const selectTab = vi.fn();
  const onOpenSession = vi.fn();
  act(() => {
    root!.render(
      <SessionStrip
        tabs={tabs}
        activeTabId={activeTabId}
        selectTab={selectTab}
        tabSelection={selectionStub()}
        tabClose={{ ...noMenu, closeSingle: vi.fn(), openMenu: vi.fn() }}
        addButtonRef={{ current: null }}
        newTab={{
          open: false,
          creating: false,
          workspaceSelected: true,
          onToggle: vi.fn(),
          onAgent: vi.fn(),
          onTerminal: vi.fn(),
          onBrowser: vi.fn(),
          onCloseMenu: vi.fn(),
        }}
        providerMenu={null}
        peerNames={new Map<string, string>()}
        resolveCreator={() => null as string | null}
        takeBackAvailable={false}
        onTakeBack={vi.fn()}
        overviewSessions={overview}
        workspaceName="atelier"
        onOpenSession={onOpenSession}
        selectedSessionId="b"
        onMoveTab={() => undefined}
      />,
    );
  });
  const trigger = (): HTMLButtonElement => {
    const element = container!.querySelector<HTMLButtonElement>(".workspace-rate");
    if (element === null) throw new Error("overview trigger did not render");
    return element;
  };
  const listbox = (): HTMLElement | null => document.querySelector<HTMLElement>("[role=listbox]");
  return { selectTab, onOpenSession, trigger, listbox };
}

describe("the overview through the strip", () => {
  it("selects the tool tab and closes through the strip", () => {
    const tabs = composeStripTabs([session("b")], [FILE, DIFF]);
    const rendered = renderStrip(tabs, ROSTER, "b");
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(rendered.listbox()).not.toBeNull();
    const row = document.querySelector<HTMLElement>(`[data-overview-option="${FILE.id}"]`)!;
    act(() => {
      row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(rendered.selectTab).toHaveBeenCalledWith(FILE.id);
    expect(rendered.onOpenSession).not.toHaveBeenCalled();
    expect(rendered.listbox()).toBeNull();
  });

  it("closes on Escape with focus back on the trigger", () => {
    const tabs = composeStripTabs([session("b")], [FILE]);
    const rendered = renderStrip(tabs, ROSTER, "b");
    act(() => {
      rendered.trigger().dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const row = document.querySelector<HTMLElement>(`[data-overview-option="${FILE.id}"]`)!;
    act(() => {
      row.focus();
      row.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(rendered.listbox()).toBeNull();
    expect(document.activeElement).toBe(rendered.trigger());
  });
});
