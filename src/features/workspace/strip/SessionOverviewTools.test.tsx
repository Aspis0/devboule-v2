// @vitest-environment happy-dom

import { act, useRef } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

import type { Session } from "../../../types/ipc";
import { SessionOverviewMenu } from "./SessionOverviewMenu";
import { composeStripTabs, makeToolTab, type StripTab } from "./toolTabs";

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

const FILE = makeToolTab("file", "workspace-1", "notes/todo.md");
const DIFF = makeToolTab("diff", "workspace-1", "src/app.ts");

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

function MenuHarness({
  tabs,
  sessions,
  stripOrder,
  activeTabId,
  activeSessionId,
  onOpen,
  onSelectTab,
  onClose,
}: {
  tabs: readonly StripTab[];
  sessions: readonly Session[];
  stripOrder: readonly string[];
  activeTabId: string | null;
  activeSessionId: string | null;
  onOpen: (id: string) => void;
  onSelectTab: (id: string) => void;
  onClose: () => void;
}) {
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
        activeTabId={activeTabId}
        activeSessionId={activeSessionId}
        workspaceName="atelier"
        onOpen={onOpen}
        onSelectTab={onSelectTab}
        onClose={onClose}
        onListEnter={() => {}}
        onListLeave={() => {}}
      />
    </>
  );
}

function renderMenu(
  tabs: readonly StripTab[],
  activeTabId: string | null = tabs[0]?.id ?? null,
  sessions: readonly Session[] = ROSTER,
  stripOrder: readonly string[] = ["b"],
) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  const calls = { onOpen: vi.fn(), onSelectTab: vi.fn(), onClose: vi.fn() };
  const state = { tabs, sessions, stripOrder, activeTabId, activeSessionId: "b" as string | null };
  const draw = () => {
    act(() => {
      root!.render(
        <MenuHarness
          tabs={state.tabs}
          sessions={state.sessions}
          stripOrder={state.stripOrder}
          activeTabId={state.activeTabId}
          activeSessionId={state.activeSessionId}
          onOpen={calls.onOpen}
          onSelectTab={calls.onSelectTab}
          onClose={calls.onClose}
        />,
      );
    });
  };
  draw();
  const option = (id: string): HTMLElement => {
    const element = document.querySelector<HTMLElement>(`[data-overview-option="${id}"]`);
    if (element === null) throw new Error(`overview option did not render: ${id}`);
    return element;
  };
  const optionOrder = (): string[] =>
    [...document.querySelectorAll<HTMLElement>("[data-overview-option]")].map(
      (element) => element.dataset.overviewOption ?? "",
    );
  const preview = (): HTMLElement => {
    const element = document.querySelector<HTMLElement>(".workspace-overview-preview");
    if (element === null) throw new Error("overview preview did not render");
    return element;
  };
  return { ...calls, option, optionOrder, preview };
}

describe("overview tool tabs", () => {
  it("lists the file and diff tabs alongside sessions in strip order", () => {
    const tabs = composeStripTabs([session("b")], [FILE, DIFF]);
    const rendered = renderMenu(tabs);
    expect(rendered.optionOrder()).toEqual(["b", FILE.id, DIFF.id, "c", "a"]);
    expect(rendered.option(FILE.id).textContent).toContain("todo.md");
    expect(rendered.option(DIFF.id).textContent).toContain("app.ts");
  });

  it("moves the preview with hover and focus for a non-session tab", () => {
    const tabs = composeStripTabs([session("b")], [FILE, DIFF]);
    const rendered = renderMenu(tabs);
    act(() => {
      rendered.option(FILE.id).dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    expect(rendered.preview().textContent).toContain("todo.md");
    expect(rendered.preview().textContent).toContain("File");
    expect(rendered.preview().textContent).toContain("notes/todo.md");
    act(() => {
      rendered.option(DIFF.id).focus();
    });
    expect(rendered.preview().textContent).toContain("app.ts");
    expect(rendered.preview().textContent).toContain("Diff");
    expect(rendered.preview().textContent).toContain("src/app.ts");
  });

  it("previews a diff row with its kind and path", () => {
    const tabs = composeStripTabs([session("b")], [DIFF]);
    const rendered = renderMenu(tabs);
    act(() => {
      rendered.option(DIFF.id).dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    expect(rendered.preview().textContent).toContain("Diff");
    expect(rendered.preview().textContent).toContain("src/app.ts");
  });

  it("groups approval requests first, then open tabs, then the rest", () => {
    const tabs = composeStripTabs([session("b")], [FILE]);
    const sessions = [
      session("b", { title: "bravo", elapsedMs: 0 }),
      session("c", {
        title: "charlie",
        state: {
          type: "recovered",
          generation: 1,
          integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
        },
        elapsedMs: null,
        attention: { reason: "permission", atMs: 1 },
      }),
      session("a", { title: "alpha", elapsedMs: 60_000 }),
    ];
    renderMenu(tabs, "b", sessions);
    const labels = [...document.querySelectorAll<HTMLElement>('[role="group"]')].map((group) =>
      group.getAttribute("aria-label"),
    );
    expect(labels).toEqual(["Needs your approval", "Open tabs", "Other sessions"]);
    const order = [...document.querySelectorAll<HTMLElement>("[data-overview-option]")].map(
      (element) => element.dataset.overviewOption ?? "",
    );
    expect(order).toEqual(["c", "b", FILE.id, "a"]);
  });

  it("marks only the active tab's row selected", () => {
    const tabs = composeStripTabs([session("b")], [FILE]);
    const rendered = renderMenu(tabs, FILE.id);
    expect(rendered.option(FILE.id).getAttribute("aria-selected")).toBe("true");
    expect(rendered.option("b").getAttribute("aria-selected")).toBe("false");
  });

  it("routes a tool row click to tab selection, never the session open path", () => {
    const tabs = composeStripTabs([session("b")], [FILE]);
    const rendered = renderMenu(tabs);
    act(() => {
      rendered.option(FILE.id).dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(rendered.onSelectTab).toHaveBeenCalledWith(FILE.id);
    expect(rendered.onOpen).not.toHaveBeenCalled();
  });

  it("selects a tool row on Enter", () => {
    const tabs = composeStripTabs([session("b")], [FILE]);
    const rendered = renderMenu(tabs);
    act(() => {
      rendered.option(FILE.id).focus();
    });
    act(() => {
      rendered
        .option(FILE.id)
        .dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    });
    expect(rendered.onSelectTab).toHaveBeenCalledWith(FILE.id);
    expect(rendered.onOpen).not.toHaveBeenCalled();
  });

  it("names a tool row by basename, then its directory, so duplicate basenames differ", () => {
    const source = makeToolTab("file", "workspace-1", "src/types/index.ts");
    const test = makeToolTab("file", "workspace-1", "tests/types/index.ts");
    const readme = makeToolTab("file", "workspace-1", "README.md");
    const rendered = renderMenu(composeStripTabs([session("b")], [source, test, readme]));
    expect(rendered.option(source.id).getAttribute("aria-label")).toBe(
      "index.ts, src/types, file tab, open tab",
    );
    expect(rendered.option(test.id).getAttribute("aria-label")).toBe(
      "index.ts, tests/types, file tab, open tab",
    );
    expect(rendered.option(readme.id).getAttribute("aria-label")).toBe(
      "README.md, file tab, open tab",
    );
  });

  it("arrows from a session row to a tool row in the open tabs group and back", () => {
    const tabs = composeStripTabs([session("b")], [FILE]);
    const rendered = renderMenu(tabs, "b");
    expect(rendered.optionOrder()).toEqual(["b", FILE.id, "c", "a"]);
    const press = (key: string) =>
      act(() => {
        document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
      });
    act(() => {
      rendered.option("b").focus();
    });
    press("ArrowDown");
    expect(document.activeElement).toBe(rendered.option(FILE.id));
    expect(rendered.option(FILE.id).tabIndex).toBe(0);
    expect(rendered.option("b").tabIndex).toBe(-1);
    press("ArrowUp");
    expect(document.activeElement).toBe(rendered.option("b"));
    expect(rendered.option("b").tabIndex).toBe(0);
  });
});
