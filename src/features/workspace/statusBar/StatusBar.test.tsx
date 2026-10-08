// @vitest-environment happy-dom

import { act, type ReactElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import { recordPlanUsage } from "../../../lib/planUsageStore";
import type { DaemonStatus, Session } from "../../../types/ipc";
import { chipDisplay } from "../strip/stripDisplay";
import { publishAgentReading, retireAgentReading } from "./agentReadingStore";
import { StatusBar } from "./StatusBar";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const DAEMON: DaemonStatus = {
  state: "connected",
  pid: 40220,
  instanceId: "d",
  protocolVersion: 21,
  clients: 1,
  capabilities: [],
  message: null,
};

const later = Math.floor(Date.now() / 1000) + 86_400;

const AGENT = {
  sessionId: "bar-agent",
  title: "Tighten handoff summary",
  attentionWord: null,
  working: true,
  quiet: false,
};

let host: HTMLDivElement | null = null;
let root: ReturnType<typeof createRoot> | null = null;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  host?.remove();
  host = null;
  root = null;
  retireAgentReading("bar-agent");
});

async function render(element: ReactElement): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(element));
  return host;
}

describe("the status bar", () => {
  it("names the focused agent and what it is on, or says it is working", async () => {
    const bar = await render(<StatusBar agent={AGENT} daemon={DAEMON} />);
    expect(bar.querySelector(".status-bar-who")?.textContent).toBe(
      "Tighten handoff summary — working",
    );

    await act(async () =>
      publishAgentReading("bar-agent", {
        usage: null,
        manifest: null,
        lastFinished: null,
        task: "running tests",
      }),
    );
    expect(bar.querySelector(".status-bar-who")?.textContent).toBe(
      "Tighten handoff summary — running tests",
    );
  });

  it("reads idle, not a stale step, when the agent is not working", async () => {
    await act(async () =>
      publishAgentReading("bar-agent", {
        usage: null,
        manifest: null,
        lastFinished: null,
        task: "running tests",
      }),
    );
    const bar = await render(<StatusBar agent={{ ...AGENT, working: false }} daemon={DAEMON} />);
    expect(bar.querySelector(".status-bar-who")?.textContent).toBe(
      "Tighten handoff summary — idle",
    );
  });

  it("says Quiet for a silent wire, the word its chip shows, even while the agent is on a step", async () => {
    await act(async () =>
      publishAgentReading("bar-agent", {
        usage: null,
        manifest: null,
        lastFinished: null,
        task: "running tests",
      }),
    );
    const bar = await render(<StatusBar agent={{ ...AGENT, quiet: true }} daemon={DAEMON} />);
    expect(bar.querySelector(".status-bar-who")?.textContent).toBe(
      "Tighten handoff summary — Quiet",
    );
  });

  it("cuts a long title to 28 characters and an ellipsis", async () => {
    const bar = await render(
      <StatusBar
        agent={{ ...AGENT, title: "abcdefghijklmnopqrstuvwxyz0123456789" }}
        daemon={DAEMON}
      />,
    );
    expect(bar.querySelector(".status-bar-title")?.textContent).toBe(
      "abcdefghijklmnopqrstuvwxyz01…",
    );
  });

  it("leaves the separator out while the title is empty", async () => {
    const bar = await render(<StatusBar agent={{ ...AGENT, title: "" }} daemon={DAEMON} />);
    expect(bar.querySelector(".status-bar-state")?.textContent).toBe("working");
    expect(bar.querySelector(".status-bar-sep")).toBeNull();
  });

  it("names a silent session with the word its chip carries", async () => {
    const session: Session = {
      id: "bar-agent",
      workspaceId: "workspace-1",
      kind: "acp",
      title: "Tighten handoff summary",
      state: { type: "silent", generation: 1 },
      elapsedMs: 90_000,
    };
    expect(chipDisplay(session).stateLine.startsWith("Quiet")).toBe(true);

    const bar = await render(<StatusBar agent={{ ...AGENT, quiet: true }} daemon={DAEMON} />);

    expect(bar.querySelector(".status-bar-state")?.textContent).toContain("Quiet");
  });

  it("says what the app waits on in place of the title while a session starts", async () => {
    const bar = await render(<StatusBar agent={AGENT} daemon={DAEMON} progress="Starting…" />);
    expect(bar.querySelector(".status-bar-state")?.textContent).toBe("Starting…");
    expect(bar.querySelector(".status-bar-title")).toBeNull();
  });

  it("mounts its live region before the progress word arrives, so the word is announced", async () => {
    const bar = await render(<StatusBar agent={null} daemon={DAEMON} />);
    const region = bar.querySelector('[role="status"]');
    expect(region?.textContent).toBe("");

    await act(async () =>
      root?.render(<StatusBar agent={null} daemon={DAEMON} progress="Loading…" />),
    );

    expect(bar.querySelector('[role="status"]')).toBe(region);
    expect(region?.textContent).toBe("Loading…");
    expect(bar.querySelector(".status-bar-state")?.textContent).toBe("Loading…");
  });

  it("puts an approval the agent waits on ahead of its task", async () => {
    const bar = await render(
      <StatusBar agent={{ ...AGENT, attentionWord: "Needs your approval" }} daemon={DAEMON} />,
    );
    expect(bar.querySelector(".status-bar-who")?.textContent).toContain("Needs your approval");
  });

  it("shows no percent for a window that has reset, and no meter once all have", async () => {
    const past = Math.floor(Date.now() / 1000) - 3600;
    recordPlanUsage({
      type: "plan_usage",
      providerId: "claude",
      windows: [
        { durationMins: 300, usedPercent: 97, resetsAt: past },
        { durationMins: 10_080, usedPercent: 41, resetsAt: Math.floor(Date.now() / 1000) + 86_400 },
      ],
    });
    recordPlanUsage({
      type: "plan_usage",
      providerId: "codex",
      windows: [{ durationMins: 300, usedPercent: 80, resetsAt: past }],
    });
    const bar = await render(<StatusBar agent={null} daemon={DAEMON} />);

    const meters = Array.from(bar.querySelectorAll(".status-bar-provider")).map(
      (meter) => meter.textContent,
    );
    expect(meters).toEqual(["Claude 41% wk"]);
    expect(bar.textContent).not.toContain("97");
  });

  it("shows each provider's usage as text and a bar, and nothing for a provider with no number", async () => {
    recordPlanUsage({
      type: "plan_usage",
      providerId: "claude",
      windows: [
        { durationMins: 300, usedPercent: 58, resetsAt: later },
        { durationMins: 10_080, usedPercent: 41, resetsAt: later },
      ],
    });
    recordPlanUsage({
      type: "plan_usage",
      providerId: "codex",
      windows: [{ durationMins: 300 }, { durationMins: 10_080, usedPercent: 37, resetsAt: later }],
    });
    recordPlanUsage({
      type: "plan_usage",
      providerId: "pi",
      windows: [{ durationMins: 300, usedPercent: 9, resetsAt: later }],
    });
    const bar = await render(<StatusBar agent={null} daemon={DAEMON} />);

    const meters = Array.from(bar.querySelectorAll(".status-bar-provider")).map(
      (meter) => meter.textContent,
    );
    expect(meters).toEqual(["Claude 58% 5h · 41% wk", "Codex 37% wk"]);
    expect(bar.querySelectorAll(".status-bar-provider .status-meter-bar")).toHaveLength(2);
    expect(bar.textContent).not.toMatch(/\bpi\b/);
  });

  it("shows the focused agent's context, and none when no reading was published", async () => {
    const bar = await render(<StatusBar agent={AGENT} daemon={DAEMON} />);
    expect(bar.querySelector(".workspace-context-meter")).toBeNull();

    await act(async () =>
      publishAgentReading("bar-agent", {
        usage: { type: "context_usage", usedTokens: 24_000, maxTokens: 1_000_000, live: true },
        manifest: null,
        lastFinished: null,
        task: null,
      }),
    );
    expect(bar.querySelector(".workspace-context-meter-text")?.textContent).toBe("ctx 24k/1m");
  });

  it("runs title, usage, context, then the daemon at the far right", async () => {
    recordPlanUsage({
      type: "plan_usage",
      providerId: "claude",
      windows: [{ durationMins: 300, usedPercent: 58, resetsAt: later }],
    });
    await act(async () =>
      publishAgentReading("bar-agent", {
        usage: { type: "context_usage", usedTokens: 24_000, maxTokens: 1_000_000, live: true },
        manifest: null,
        lastFinished: null,
        task: null,
      }),
    );
    const bar = await render(<StatusBar agent={AGENT} daemon={DAEMON} />);

    // The live region draws nothing, so it is not one of the bar's items.
    const order = Array.from(bar.querySelector(".workspace-status-bar")?.children ?? [])
      .filter((child) => child.getAttribute("role") !== "status")
      .map((child) => child.className);
    // Usage meters come from the shared store, so earlier cases may leave more
    // than one provider: the order is what this case pins.
    const providers = order.filter((name) => name === "status-bar-provider").length;
    expect(providers).toBeGreaterThan(0);
    expect(order).toEqual([
      "status-bar-who",
      ...Array<string>(providers).fill("status-bar-provider"),
      "workspace-context-meter",
      "status-bar-spacer",
      "status-bar-daemon",
    ]);
  });

  it("ends with the daemon, said to a screen reader and a tooltip, with no agent in front", async () => {
    const bar = await render(<StatusBar agent={null} daemon={DAEMON} />);
    expect(bar.querySelector(".status-bar-who")).toBeNull();
    const dot = bar.querySelector(".status-bar-daemon");
    // One live region for the daemon: the rail's. The bar's dot is a report, not a second announcement.
    expect(dot?.hasAttribute("role")).toBe(false);
    expect(dot?.getAttribute("title")).toBe("daemon · pid 40220");
    expect(dot?.querySelector(".sr-only")?.textContent).toBe("daemon · pid 40220");
    expect(bar.querySelector(".workspace-status-bar")?.lastElementChild).toBe(dot);
  });
});
