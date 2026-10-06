// @vitest-environment happy-dom

import { act, type ReactElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import { recordPlanUsage } from "../../../lib/planUsageStore";
import type { DaemonStatus } from "../../../types/ipc";
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

const AGENT = {
  sessionId: "bar-agent",
  title: "Tighten handoff summary",
  attentionWord: null,
  working: true,
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
        { durationMins: 300, usedPercent: 58 },
        { durationMins: 10_080, usedPercent: 41 },
      ],
    });
    recordPlanUsage({
      type: "plan_usage",
      providerId: "codex",
      windows: [{ durationMins: 300 }, { durationMins: 10_080, usedPercent: 37 }],
    });
    recordPlanUsage({
      type: "plan_usage",
      providerId: "pi",
      windows: [{ durationMins: 300, usedPercent: 9 }],
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
