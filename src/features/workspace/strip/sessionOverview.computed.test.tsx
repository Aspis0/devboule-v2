// @vitest-environment happy-dom

// The overview's geometry against the real stylesheets: the list's scroll
// cap, the popover's flex column, the order-proof padding, and the keyboard
// ring — all read from the assembled sheets, never from a shape assertion.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MouseEvent as ReactMouseEvent } from "react";
import type { Session } from "../../../types/ipc";
import { SessionStrip } from "./SessionStrip";
import { composeStripTabs } from "./toolTabs";
import { assembleCssProof, removeCssProof } from "../cssProof";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

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

let root: ReturnType<typeof createRoot> | null = null;
let container: HTMLDivElement | null = null;

function renderOpenStrip() {
  const roster = [session("a"), session("b")];
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root!.render(
      <SessionStrip
        tabs={composeStripTabs([roster[0]!], [])}
        activeTabId={roster[0]!.id}
        selectTab={vi.fn()}
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
          onCloseMenu: vi.fn(),
        }}
        providerMenu={null}
        peerNames={new Map<string, string>()}
        resolveCreator={() => null as string | null}
        takeBackAvailable={false}
        onTakeBack={vi.fn()}
        statusText="1 sessions"
        overviewSessions={roster}
        workspaceName="atelier"
        onOpenSession={vi.fn()}
        selectedSessionId={roster[0]!.id}
      />,
    );
  });
  const trigger = container.querySelector<HTMLElement>(".workspace-rate")!;
  act(() => {
    trigger.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

afterEach(() => {
  act(() => {
    root?.unmount();
  });
  container?.remove();
  root = null;
  container = null;
  removeCssProof();
  vi.clearAllMocks();
});

describe("overview computed styles (real stylesheets, no app launch)", () => {
  // Both sheet orders: the overview padding must not depend on which sheet
  // the bundle emits last.
  const sheetsFirst = [
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/workspace/strip/strip.css"),
    read("src/features/workspace/Workspace.css"),
  ];
  const sheetsSwapped = [
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/workspace/Workspace.css"),
    read("src/features/workspace/strip/strip.css"),
  ];

  it("scrolls the list inside its cap with the preview below it", () => {
    assembleCssProof(sheetsFirst).inject([
      ".workspace-overview",
      ".workspace-overview-list",
      ".workspace-overview-preview",
    ]);
    renderOpenStrip();
    const popover = document.querySelector<HTMLElement>(".workspace-overview")!;
    expect(getComputedStyle(popover).display).toBe("flex");
    const list = document.querySelector<HTMLElement>(".workspace-overview-list")!;
    expect(getComputedStyle(list).maxHeight).toBe("240px");
    expect(getComputedStyle(list).overflowY).toBe("auto");
    expect(document.querySelector(".workspace-overview-preview")).not.toBeNull();
  });

  it.each([
    ["strip first", sheetsFirst],
    ["workspace first", sheetsSwapped],
  ])("keeps the 4px overview padding with %s", (_label, sheets) => {
    assembleCssProof(sheets).inject([
      ".workspace-surface-menu",
      ".workspace-surface-menu.workspace-overview",
    ]);
    renderOpenStrip();
    const popover = document.querySelector<HTMLElement>(".workspace-overview")!;
    expect(getComputedStyle(popover).paddingTop).toBe("4px");
  });

  it("rings the focused option inside its own box", () => {
    const { rulesFor } = assembleCssProof(sheetsFirst);
    const hover = rulesFor(".workspace-overview-option:hover");
    // The hover rule must exist for the negative check below to mean
    // anything: it carries the shared fill, without any outline.
    expect(hover).toContain("background");
    expect(hover).not.toContain("outline");
    const focus = rulesFor(".workspace-overview-option:focus-visible");
    expect(focus).toContain("outline:");
    expect(focus).toContain("-2px");
    expect(focus).not.toContain("outline: none");
  });
});
