// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AgentRows, type AgentRowsProps } from "./AgentRows";
import type { RailAgent, RailAgents } from "./railAgents";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const row = (over: Partial<RailAgent> = {}): RailAgent => ({
  id: "session",
  kind: "claude",
  provider: "Claude",
  title: "Kalsa design concepts",
  state: "running",
  child: false,
  parentTitle: null,
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
    ...over,
  };
  await act(async () => root.render(<AgentRows {...props} />));
  return props;
}

const rowButtons = () => [...container.querySelectorAll<HTMLButtonElement>("button.agent-row")];

describe("the agent rows under a workspace", () => {
  it("draws nothing for a workspace with no agents", async () => {
    await render({}, { rows: [], hidden: 0 });
    expect(container.querySelector(".agent-rows")).toBeNull();
  });

  it("nests a child one step in", async () => {
    await render(
      {},
      {
        rows: [
          row({ id: "root", title: "cia" }),
          row({ id: "child", title: "Website redesign", child: true, parentTitle: "cia" }),
        ],
      },
    );
    const [parent, child] = rowButtons();
    expect(parent?.classList.contains("agent-row-child")).toBe(false);
    expect(child?.classList.contains("agent-row-child")).toBe(true);
  });

  it.each([
    ["running", "agent-row-badge-running", null],
    ["waiting", "agent-row-badge-waiting", "needs you"],
    ["failed", "agent-row-badge-failed", "failed"],
    ["stopped", null, "stopped"],
    ["finished", null, null],
    ["idle", null, null],
  ] as const)("marks a %s agent with its badge and tail", async (state, badge, tail) => {
    await render({}, { rows: [row({ state })] });
    const button = rowButtons()[0];
    const badgeEl = button?.querySelector(".agent-row-badge") ?? null;
    expect(badgeEl === null).toBe(badge === null);
    if (badge !== null) expect(badgeEl?.classList.contains(badge)).toBe(true);
    expect(button?.querySelector(".agent-row-tail")?.textContent ?? null).toBe(tail);
  });

  it("names the agent, its state, its provider and its parent for a screen reader", async () => {
    await render(
      {},
      { rows: [row({ state: "waiting", child: true, parentTitle: "cia", provider: "Codex" })] },
    );
    expect(rowButtons()[0]?.getAttribute("aria-label")).toBe(
      "Kalsa design concepts, needs you, Codex, child of cia",
    );
  });

  it("opens the agent whose row is clicked", async () => {
    const props = await render({}, { rows: [row({ id: "session-7" })] });
    await act(async () => rowButtons()[0]?.click());
    expect(props.onOpenAgent).toHaveBeenCalledWith("session-7");
  });

  it("marks the row of the agent in front as current", async () => {
    await render(
      { activeId: "session-7" },
      { rows: [row({ id: "session-7" }), row({ id: "other" })] },
    );
    const [front, other] = rowButtons();
    expect(front?.getAttribute("aria-current")).toBe("true");
    expect(other?.getAttribute("aria-current")).toBeNull();
  });

  it("ends with the count of rows past the cap, which opens History", async () => {
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
});
