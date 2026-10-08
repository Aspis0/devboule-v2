// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { StrictMode, act, type ReactNode, type RefObject } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { channelHarness } from "./sessionChannelHarness";
import { AgentChatSurface } from "./AgentChatSurface";

vi.mock("../../lib/tauri", async () => (await import("./sessionChannelHarness")).tauriMock);

// The menu renders through the shared anchored portal, whose placement
// arithmetic is pinned on its own pure inputs in popoverPlace.test.ts —
// happy-dom does no layout, so here the portal is a passthrough that
// records the one prop this slice chooses (the upward preference) and
// hands its root back through containerRef, which the real portal does
// and the focus-return branch needs.
vi.mock("./popoverPlace", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./popoverPlace")>();
  return {
    ...actual,
    AnchoredPopover: (props: {
      anchorRef?: RefObject<HTMLElement | null>;
      containerRef?: RefObject<HTMLDivElement | null>;
      onDismiss?: () => void;
      openAbove?: boolean;
      children?: ReactNode;
      className?: string;
      id?: string;
    }) => (
      <div
        ref={(node: HTMLDivElement | null) => {
          if (props.containerRef) props.containerRef.current = node;
        }}
        data-open-above={props.openAbove === true ? "true" : "false"}
        tabIndex={-1}
        className={props.className}
        id={props.id}
      >
        {props.children}
      </div>
    ),
  };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("SubagentMenu pill words", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root?.unmount());
    container.remove();
    channelHarness.emit = null;
    channelHarness.active = null;
    channelHarness.activeSubscriptionId = null;
  });

  it("reads N failed and N working, hiding a zero part", async () => {
    await act(async () => {
      root.render(
        <StrictMode>
          <AgentChatSurface daemonState="connected" sessionId="pill-words" />
        </StrictMode>,
      );
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-failed-1",
        title: "Failed task",
        subagentType: "verifier",
        toolUseId: "toolu-failed-1",
        spawnDepth: 1,
      });
      channelHarness.active?.({
        type: "agent_task_notification",
        taskId: "task-failed-1",
        status: "failed",
      });
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-running-1",
        title: "Running task",
        subagentType: "worker",
        toolUseId: "toolu-running-1",
        spawnDepth: 1,
      });
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-running-2",
        title: "Another running task",
        subagentType: "worker",
        toolUseId: "toolu-running-2",
        spawnDepth: 1,
      });
    });

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    expect(pill.textContent).toContain("1 failed");
    expect(pill.textContent).toContain("2 working");
    // A zero part is hidden, not rendered as a zero count: two groups, and
    // neither count reads 0.
    expect(pill.querySelectorAll(".workspace-subagent-pill-group")).toHaveLength(2);
    expect(pill.textContent).not.toContain("0 failed");
    expect(pill.textContent).not.toContain("0 working");
  });

  it("names itself for a screen reader from the counts it carries", async () => {
    await act(async () => {
      root.render(
        <StrictMode>
          <AgentChatSurface daemonState="connected" sessionId="pill-name" />
        </StrictMode>,
      );
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-failed-1",
        title: "Failed task",
        subagentType: "verifier",
        toolUseId: "toolu-failed-1",
        spawnDepth: 1,
      });
      channelHarness.active?.({
        type: "agent_task_notification",
        taskId: "task-failed-1",
        status: "failed",
      });
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-running-1",
        title: "Running task",
        subagentType: "worker",
        toolUseId: "toolu-running-1",
        spawnDepth: 1,
      });
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-running-2",
        title: "Another running task",
        subagentType: "worker",
        toolUseId: "toolu-running-2",
        spawnDepth: 1,
      });
    });

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    expect(pill.getAttribute("aria-label")).toBe("Subagents: 1 failed, 2 working");
  });

  it("counts a child waiting on an approval card only as needing approval, never as working", async () => {
    await act(async () => {
      root.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId="parent"
          sessionRoster={[
            {
              id: "child-waiting",
              kind: "acp",
              title: "Waiting",
              createdBy: "parent",
              state: { type: "live", generation: 1 },
              activity: "blocked",
            },
            {
              id: "child-running",
              kind: "acp",
              title: "Running",
              createdBy: "parent",
              state: { type: "live", generation: 1 },
              activity: "working",
            },
          ]}
          subagentAttention={new Map([["child-waiting", "Needs your approval"]])}
        />,
      );
    });
    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    expect(pill?.getAttribute("aria-label")).toBe("Subagents: 1 needs your approval, 1 working");
  });

  it("sizes its dots with its own rule, not the shared 7 px one", async () => {
    await act(async () => {
      root.render(
        <StrictMode>
          <AgentChatSurface daemonState="connected" sessionId="pill-dot" />
        </StrictMode>,
      );
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-failed-1",
        title: "Failed task",
        subagentType: "verifier",
        toolUseId: "toolu-failed-1",
        spawnDepth: 1,
      });
      channelHarness.active?.({
        type: "agent_task_notification",
        taskId: "task-failed-1",
        status: "failed",
      });
    });

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    // The shared .workspace-status-dot (Workspace.css) is 7 px and loads
    // after the subagent sheet; these dots carry only their own rule.
    expect(pill.querySelector(".workspace-status-dot")).toBeNull();
    expect(pill.querySelector(".workspace-subagent-status-dot")).not.toBeNull();
  });

  it("breathes only while something works, and drops the dot with the last completion", async () => {
    await act(async () => {
      root.render(
        <StrictMode>
          <AgentChatSurface daemonState="connected" sessionId="breathing-dot" />
        </StrictMode>,
      );
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-running-1",
        title: "Running task",
        subagentType: "worker",
        toolUseId: "toolu-running-1",
        spawnDepth: 1,
      });
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-running-2",
        title: "Another running task",
        subagentType: "worker",
        toolUseId: "toolu-running-2",
        spawnDepth: 1,
      });
    });

    let pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    expect(pill.querySelector(".dot-pulse")).not.toBeNull();

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_notification",
        taskId: "task-running-1",
        status: "completed",
      });
    });

    pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    expect(pill.querySelector(".dot-pulse")).not.toBeNull();

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_notification",
        taskId: "task-running-2",
        status: "completed",
      });
    });

    pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    // Nothing is working any more: the breathing dot goes with the last
    // completion. The pill stays, naming the run it covers.
    expect(pill.querySelector(".dot-pulse")).toBeNull();
    expect(pill.textContent).toContain("2 subagents");
  });

  it("names the run it covers in the singular, once its only child has settled", async () => {
    await act(async () => {
      root.render(
        <StrictMode>
          <AgentChatSurface daemonState="connected" sessionId="pill-one" />
        </StrictMode>,
      );
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-only",
        title: "Only task",
        subagentType: "worker",
        toolUseId: "toolu-only",
        spawnDepth: 1,
      });
      channelHarness.active?.({
        type: "agent_task_notification",
        taskId: "task-only",
        status: "completed",
      });
    });

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    // A chevron alone tells the user nothing about what the menu opens, so
    // the settled pill paints the total it covers — and says it aloud.
    expect(pill.textContent).toContain("1 subagent");
    expect(pill.textContent).not.toContain("1 subagents");
    expect(pill.getAttribute("aria-label")).toBe("1 subagent");
    expect(pill.querySelectorAll(".workspace-subagent-pill-group")).toHaveLength(1);
    expect(pill.querySelector(".workspace-subagent-status-dot")).toBeNull();
    expect(pill.querySelector(".workspace-subagent-pill-chevron")).not.toBeNull();
  });

  it("keeps the pill when the run has settled, and opens its menu", async () => {
    await act(async () => {
      root.render(
        <StrictMode>
          <AgentChatSurface daemonState="connected" sessionId="pill-idle" />
        </StrictMode>,
      );
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-done",
        title: "Finished task",
        subagentType: "worker",
        toolUseId: "toolu-done",
        spawnDepth: 1,
      });
      channelHarness.active?.({
        type: "agent_task_notification",
        taskId: "task-done",
        status: "completed",
      });
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-halted",
        title: "Stopped task",
        subagentType: "worker",
        toolUseId: "toolu-halted",
        spawnDepth: 1,
      });
      channelHarness.active?.({
        type: "agent_task_notification",
        taskId: "task-halted",
        status: "stopped",
      });
    });

    // A settled run keeps the pill — the only place the app names a
    // subagent — naming the total it covers, with the breathing dot gone.
    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    expect(pill.querySelectorAll(".workspace-subagent-pill-group")).toHaveLength(1);
    expect(pill.querySelector(".workspace-subagent-status-dot")).toBeNull();
    expect(pill.querySelector(".dot-pulse")).toBeNull();
    expect(pill.textContent).toContain("2 subagents");
    expect(pill.getAttribute("aria-label")).toBe("2 subagents");

    await act(async () => pill.click());
    const list = container.querySelector<HTMLElement>(".workspace-subagent-list");
    expect(list).not.toBeNull();
    expect(list?.querySelectorAll(".workspace-subagent-row")).toHaveLength(2);
    expect(list?.textContent).toContain("Finished task");
    expect(list?.textContent).toContain("Stopped task");
  });
});

describe("SubagentMenu list", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root?.unmount());
    container.remove();
    channelHarness.emit = null;
    channelHarness.active = null;
    channelHarness.activeSubscriptionId = null;
  });

  it("has a head and rows, and closes on Escape, returning focus the menu held", async () => {
    await act(async () => {
      root.render(
        <StrictMode>
          <AgentChatSurface
            daemonState="connected"
            sessionId="list-escape"
            onOpenSubagent={() => undefined}
            sessionRoster={[
              {
                id: "task-child",
                kind: "acp",
                title: "Child task",
                createdBy: "list-escape",
                state: { type: "live", generation: 1 },
              },
            ]}
          />
        </StrictMode>,
      );
    });
    await act(async () => undefined);

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    await act(async () => pill.click());

    const list = container.querySelector<HTMLElement>(".workspace-subagent-list");
    expect(list).not.toBeNull();
    expect(list?.querySelector(".workspace-subagent-list-head")?.textContent).toBe("Subagents");
    expect(list?.querySelectorAll(".workspace-subagent-row")).toHaveLength(1);
    expect(list?.textContent).toContain("Child task");

    list?.querySelector<HTMLButtonElement>("button")?.focus();
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(container.querySelector(".workspace-subagent-list")).toBeNull();
    expect(document.activeElement).toBe(pill);
  });

  it("asks the portal for the upward preference — the portal picks the side with room", async () => {
    await act(async () => {
      root.render(
        <StrictMode>
          <AgentChatSurface daemonState="connected" sessionId="list-upward" />
        </StrictMode>,
      );
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-child",
        title: "Child task",
        subagentType: "worker",
        toolUseId: "toolu-child",
        spawnDepth: 1,
      });
    });

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    await act(async () => pill.click());

    // The pill asks for "above" as a preference; whether the menu fits there
    // is the portal's call, pinned on its own inputs in
    // popoverPlace.test.ts — from the pane header the answer is below,
    // over the transcript.
    const opened = container.querySelector<HTMLElement>('[data-open-above="true"]');
    expect(opened).not.toBeNull();
    expect(opened?.classList.contains("workspace-subagent-list")).toBe(true);
    expect(opened?.querySelector(".workspace-subagent-list-head")?.textContent).toBe("Subagents");
  });

  it("keeps the head a sibling of the list, and gives each row the spec's chevron", async () => {
    await act(async () => {
      root.render(
        <StrictMode>
          <AgentChatSurface daemonState="connected" sessionId="list-structure" />
        </StrictMode>,
      );
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-child",
        title: "Child task",
        subagentType: "worker",
        toolUseId: "toolu-child",
        spawnDepth: 1,
      });
    });

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    await act(async () => pill.click());

    const list = container.querySelector('[role="list"]');
    expect(list).not.toBeNull();
    // role="list" owns only listitem children; the head sits beside it, not
    // inside it to be announced as one.
    expect(list?.querySelector(".workspace-subagent-list-head")).toBeNull();
    expect(list?.parentElement?.querySelector(".workspace-subagent-list-head")?.textContent).toBe(
      "Subagents",
    );
    for (const child of Array.from(list?.children ?? [])) {
      expect(child.getAttribute("role")).toBe("listitem");
    }
    // The spec's row is dot + name + chevron (SPEC-regions:121); the
    // chevron keeps the house stroke convention, not a filled path.
    const rows = list?.querySelectorAll(".workspace-subagent-row");
    expect(rows).toHaveLength(1);
    for (const row of Array.from(rows ?? [])) {
      const chevron = row.querySelector<SVGSVGElement>("svg.workspace-subagent-row-chevron");
      expect(chevron).not.toBeNull();
      expect(chevron?.getAttribute("aria-hidden")).toBe("true");
    }
  });

  it("treats a press on the pill as the pill's own, not an outside press", async () => {
    await act(async () => {
      root.render(
        <StrictMode>
          <AgentChatSurface daemonState="connected" sessionId="list-pill-press" />
        </StrictMode>,
      );
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-child",
        title: "Child task",
        subagentType: "worker",
        toolUseId: "toolu-child",
        spawnDepth: 1,
      });
    });

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    await act(async () => pill.click());
    expect(container.querySelector(".workspace-subagent-list")).not.toBeNull();

    // A press on the pill while the menu is open belongs to the pill's own
    // toggle: the outside-press handler must leave it alone.
    await act(async () => {
      pill.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    });
    expect(container.querySelector(".workspace-subagent-list")).not.toBeNull();

    // The pill's own toggle then closes it.
    await act(async () => pill.click());
    expect(container.querySelector(".workspace-subagent-list")).toBeNull();
  });

  it("opens on the keyboard — Enter and Space both reach the menu", async () => {
    await act(async () => {
      root.render(
        <StrictMode>
          <AgentChatSurface daemonState="connected" sessionId="list-keyboard" />
        </StrictMode>,
      );
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_task_started",
        taskId: "task-child",
        title: "Child task",
        subagentType: "worker",
        toolUseId: "toolu-child",
        spawnDepth: 1,
      });
    });

    const pill = container.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    if (pill === null) throw new Error("subagent pill did not render");
    // A browser activates a button on Enter (keydown) and Space (keyup)
    // with a click; happy-dom implements no activation behaviour, so each
    // key is followed by the click it would have produced.
    expect(pill.tagName).toBe("BUTTON");
    pill.focus();
    await act(async () => {
      pill.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
      );
      pill.click();
    });
    expect(container.querySelector(".workspace-subagent-list")).not.toBeNull();
    expect(pill.getAttribute("aria-expanded")).toBe("true");

    await act(async () => pill.click());
    expect(container.querySelector(".workspace-subagent-list")).toBeNull();

    await act(async () => {
      pill.dispatchEvent(
        new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true }),
      );
      pill.dispatchEvent(new KeyboardEvent("keyup", { key: " ", bubbles: true, cancelable: true }));
      pill.click();
    });
    expect(container.querySelector(".workspace-subagent-list")).not.toBeNull();
    expect(pill.getAttribute("aria-expanded")).toBe("true");
  });
});

const css = readFileSync(resolve(import.meta.dirname, "SubagentMenu.css"), "utf8");

/** The body of the top-level rule whose selector is exactly `selector`. */
function ruleBody(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const body = css.match(new RegExp(`^${escaped}\\s*\\{([\\s\\S]*?)\\n\\}`, "m"))?.[1];
  if (body === undefined) throw new Error(`no rule in SubagentMenu.css for ${selector}`);
  return body;
}

describe("the subagent sheet", () => {
  it("draws the pill chevron as a stroke, not a filled path", () => {
    // An SVG path defaults to fill black and stroke none — a black wedge
    // on --panel-menu, invisible against the dark theme's ground. The house
    // icon convention (.ic, .workspace-mode-caret) is fill none +
    // currentColor.
    const chevron = ruleBody(".workspace-subagent-pill-chevron");
    expect(chevron).toContain("fill: none;");
    expect(chevron).toContain("stroke: currentColor;");
  });

  it("draws the row chevron as a stroke, not a filled path", () => {
    // The row caret is an icon too: a default-filled path would be a black
    // wedge on the row's ground. Same house rule as the pill's chevron.
    const chevron = ruleBody(".workspace-subagent-row-chevron");
    expect(chevron).toContain("fill: none;");
    expect(chevron).toContain("stroke: currentColor;");
  });

  it("highlights rows on hover and keyboard focus", () => {
    expect(css).toContain(".workspace-subagent-row:hover");
    expect(css).toContain(".workspace-subagent-row:focus-visible");
  });

  it("sizes its dots at the mockup's 6 px with the one rule that governs them", () => {
    const dot = ruleBody(".workspace-subagent-status-dot");
    expect(dot).toContain("width: 6px;");
    expect(dot).toContain("height: 6px;");
    expect(dot).toContain("border-radius: var(--radius-full);");
  });
});
