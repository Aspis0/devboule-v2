// @vitest-environment happy-dom

import { act, type ReactNode, type RefObject } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AgentRows, type AgentRowsProps } from "./AgentRows";
import type { RailAgent, RailAgents } from "./railAgents";

vi.mock("../popoverPlace", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../popoverPlace")>();
  return {
    ...actual,
    AnchoredPopover: (props: {
      containerRef?: RefObject<HTMLDivElement | null>;
      className?: string;
      children?: ReactNode;
    }) => (
      <div
        ref={(node: HTMLDivElement | null) => {
          if (props.containerRef) props.containerRef.current = node;
        }}
        className={props.className}
      >
        {props.children}
      </div>
    ),
  };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const row = (over: Partial<RailAgent> = {}): RailAgent => ({
  id: "session",
  kind: "claude",
  provider: "Claude",
  title: "Kalsa design concepts",
  state: "running",
  child: false,
  parentId: null,
  parentTitle: null,
  hasChildren: false,
  ...over,
});

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

async function render(over: Partial<AgentRowsProps> = {}, agents: Partial<RailAgents> = {}) {
  const props: AgentRowsProps = {
    agents: { rows: [row()], hidden: 0, ...agents },
    activeId: null,
    onOpenAgent: vi.fn(),
    onOpenHistory: vi.fn(),
    onArchiveAgent: vi.fn(async () => null),
    ...over,
  };
  await act(async () => root.render(<AgentRows {...props} />));
  return props;
}

const items = () => [...container.querySelectorAll<HTMLElement>('[role="treeitem"]')];

const keyDown = async (target: HTMLElement, key: string, init: KeyboardEventInit = {}) => {
  await act(async () => {
    target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, ...init }));
  });
};

describe("the agent rows under a workspace", () => {
  it("draws nothing for a workspace with no agents", async () => {
    await render({}, { rows: [], hidden: 0 });
    expect(container.querySelector(".agent-rows")).toBeNull();
  });

  it("is one tree, each agent a tree item with its level", async () => {
    await render(
      {},
      {
        rows: [
          row({ id: "root", title: "cia", hasChildren: true }),
          row({
            id: "child",
            title: "Website redesign",
            child: true,
            parentId: "root",
            parentTitle: "cia",
          }),
        ],
      },
    );
    expect(container.querySelector('[role="tree"]')).not.toBeNull();
    expect(items().map((item) => item.getAttribute("aria-level"))).toEqual(["1", "2"]);
    expect(items()[0]?.getAttribute("aria-expanded")).toBe("true");
    expect(items()[1]?.hasAttribute("aria-expanded")).toBe(false);
  });

  it("gives one row the tab stop, the front agent when it is listed", async () => {
    await render({ activeId: "second" }, { rows: [row({ id: "first" }), row({ id: "second" })] });
    expect(items().map((item) => item.tabIndex)).toEqual([-1, 0]);
    expect(items()[1]?.getAttribute("aria-current")).toBe("true");
  });

  it.each([
    ["running", "agent-row-badge-running", null],
    ["waiting", "agent-row-badge-waiting", "needs you"],
    ["finished", null, null],
    ["idle", null, null],
  ] as const)("marks a %s agent with its badge and tail", async (state, badge, tail) => {
    await render({}, { rows: [row({ state })] });
    const item = items()[0];
    const badgeEl = item?.querySelector(".agent-row-badge") ?? null;
    expect(badgeEl === null).toBe(badge === null);
    if (badge !== null) expect(badgeEl?.classList.contains(badge)).toBe(true);
    expect(item?.querySelector(".agent-row-tail")?.textContent ?? null).toBe(tail);
  });

  it("names the agent, its state, its provider and its parent for a screen reader", async () => {
    await render(
      {},
      { rows: [row({ state: "waiting", child: true, parentTitle: "cia", provider: "Codex" })] },
    );
    expect(items()[0]?.getAttribute("aria-label")).toBe(
      "Kalsa design concepts, needs you, Codex, child of cia",
    );
  });

  it("opens the agent whose row is clicked", async () => {
    const props = await render({}, { rows: [row({ id: "session-7" })] });
    await act(async () => items()[0]?.click());
    expect(props.onOpenAgent).toHaveBeenCalledWith("session-7");
  });

  it("opens the agent on Enter, and moves focus with the arrows", async () => {
    const props = await render(
      {},
      { rows: [row({ id: "a" }), row({ id: "b", child: true, parentId: "a", parentTitle: "A" })] },
    );
    items()[0]?.focus();
    await keyDown(items()[0] as HTMLElement, "ArrowDown");
    expect(document.activeElement).toBe(items()[1]);
    expect(items()[1]?.tabIndex).toBe(0);
    await keyDown(items()[1] as HTMLElement, "Enter");
    expect(props.onOpenAgent).toHaveBeenCalledWith("b");
  });

  it("shows the cap's count and opens History from it", async () => {
    const props = await render({}, { rows: [row()], hidden: 3 });
    const more = container.querySelector<HTMLButtonElement>(".agent-row-more");
    expect(more?.textContent).toBe("+3 more in History");
    await act(async () => more?.click());
    expect(props.onOpenHistory).toHaveBeenCalledTimes(1);
  });

  it("shows no more-row when nothing waits behind the cap", async () => {
    await render({}, { rows: [row()], hidden: 0 });
    expect(container.querySelector(".agent-row-more")).toBeNull();
  });

  it("opens the row menu from the keyboard only on a finished agent, which can be archived", async () => {
    await render(
      {},
      { rows: [row({ id: "done", state: "finished", child: true, parentId: "x" })] },
    );
    await keyDown(items()[0] as HTMLElement, "F10", { shiftKey: true });
    expect(container.querySelector('[role="menu"]')).not.toBeNull();
  });

  it("does not open a menu on a running agent", async () => {
    await render({}, { rows: [row({ state: "running" })] });
    await keyDown(items()[0] as HTMLElement, "F10", { shiftKey: true });
    expect(container.querySelector('[role="menu"]')).toBeNull();
  });
});
