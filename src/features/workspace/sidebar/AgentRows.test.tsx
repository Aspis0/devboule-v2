// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { publishAgentReading, retireAgentReading } from "../statusBar/agentReadingStore";
import type { AgentRowView } from "./agentRowViews";
import { AgentRows } from "./AgentRows";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const agent = (over: Partial<AgentRowView> = {}): AgentRowView => ({
  id: "a-1",
  kind: "claude",
  title: "Tighten handoff summary",
  word: "idle",
  attention: false,
  working: false,
  age: "1h",
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
  retireAgentReading("a-1");
});

async function render(
  agents: AgentRowView[],
  active: string | null = null,
  onOpen = vi.fn(),
): Promise<void> {
  await act(async () =>
    root.render(<AgentRows agents={agents} activeSessionId={active} onOpen={onOpen} />),
  );
}

describe("the agents under a workspace", () => {
  it("draws nothing for a workspace with no agents", async () => {
    await render([]);
    expect(container.querySelector(".workspace-agent-rows")).toBeNull();
  });

  it("shows the mark, the name, and the state with its age", async () => {
    await render([agent()]);

    const row = container.querySelector(".workspace-agent-row");
    expect(row?.querySelector(".strip-kind")).not.toBeNull();
    expect(row?.querySelector(".workspace-agent-name")?.textContent).toBe(
      "Tighten handoff summary",
    );
    expect(row?.querySelector(".workspace-agent-sub")?.textContent).toBe("idle · 1h");
    // One name for a screen reader, built from the same words.
    expect(row?.getAttribute("aria-label")).toBe("Tighten handoff summary, idle · 1h");
  });

  it("says the step a working agent is on when its surface published one, else working", async () => {
    await render([agent({ word: "working", working: true, age: "now" })]);
    expect(container.querySelector(".workspace-agent-sub")?.textContent).toBe("working");

    await act(async () =>
      publishAgentReading("a-1", {
        usage: null,
        manifest: null,
        lastFinished: null,
        task: "running tests",
      }),
    );
    expect(container.querySelector(".workspace-agent-sub")?.textContent).toBe("running tests");
  });

  it("gives an ask for approval the attention tone", async () => {
    await render([agent({ word: "Needs your approval", attention: true })]);
    expect(
      container.querySelector(".workspace-agent-sub")?.classList.contains("sidebar-row-waiting"),
    ).toBe(true);
  });

  it("marks the agent whose tab is in front, and opens an agent on a click", async () => {
    const onOpen = vi.fn();
    await render([agent({ id: "a-1" }), agent({ id: "a-2", title: "Draft notes" })], "a-2", onOpen);

    const rows = container.querySelectorAll<HTMLButtonElement>(".workspace-agent-row");
    expect(rows[0]?.getAttribute("aria-current")).toBeNull();
    expect(rows[1]?.getAttribute("aria-current")).toBe("true");
    expect(rows[1]?.classList.contains("workspace-agent-row-active")).toBe(true);

    await act(async () => rows[0]?.click());
    expect(onOpen).toHaveBeenCalledWith("a-1");
  });

  it("is reachable by keyboard: every agent is a real button", async () => {
    await render([agent()]);
    expect(container.querySelector(".workspace-agent-row")?.tagName).toBe("BUTTON");
  });
});
