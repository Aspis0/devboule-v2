// @vitest-environment happy-dom

// The usage line's answers: a missing reading is said, a percent is tinted only
// near its limit, and the daemon's restart note joins the daemon's tooltip.

import { act, type ReactElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import { recordPlanUsage } from "../../../lib/planUsageStore";
import type { DaemonStatus } from "../../../types/ipc";
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

const later = Math.floor(Date.now() / 1000) + 7_200;

const AGENT = {
  sessionId: "usage-agent",
  title: "Tighten handoff summary",
  attentionWord: null,
  attentionTone: null,
  working: true,
  stateWord: null,
};

let host: HTMLDivElement | null = null;
let root: ReturnType<typeof createRoot> | null = null;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  host?.remove();
  host = null;
  root = null;
});

async function render(element: ReactElement): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(element));
  return host;
}

describe("the status bar's usage line", () => {
  it("says a limit-reporting provider has no reading yet, instead of showing nothing", async () => {
    const bar = await render(
      <StatusBar agent={{ ...AGENT, providerId: "claude" }} daemon={DAEMON} />,
    );

    const line = bar.querySelector(".status-bar-unreported");
    expect(line?.textContent).toBe("Claude usage not reported yet");
    expect(line?.getAttribute("title")).toBe(
      "Claude has not reported a usage window yet. It appears here once it does.",
    );
  });

  it("shows nothing for a provider that never reports plan limits", async () => {
    const bar = await render(
      <StatusBar agent={{ ...AGENT, providerId: "gemini" }} daemon={DAEMON} />,
    );

    expect(bar.querySelector(".status-bar-provider")).toBeNull();
  });

  it("tints the bar amber from 70 percent and red past 90, and leaves a lower reading neutral", async () => {
    const percentFill = async (usedPercent: number) => {
      recordPlanUsage({
        type: "plan_usage",
        providerId: "codex",
        windows: [{ durationMins: 300, usedPercent, resetsAt: later }],
      });
      const bar = await render(
        <StatusBar agent={{ ...AGENT, providerId: "codex" }} daemon={DAEMON} />,
      );
      const fill = bar.querySelector(".status-meter-fill");
      if (fill === null) throw new Error("no fill drawn");
      return fill.className;
    };

    expect(await percentFill(45)).toBe("status-meter-fill");
    expect(await percentFill(75)).toBe("status-meter-fill status-meter-fill-warn");
    expect(await percentFill(95)).toBe("status-meter-fill status-meter-fill-danger");
  });

  it("appends the restart note to the daemon tooltip, after the daemon's own sentence", async () => {
    const bar = await render(
      <StatusBar agent={AGENT} daemon={DAEMON} daemonNote="The daemon restart failed." />,
    );

    expect(bar.querySelector(".status-bar-daemon")?.getAttribute("title")).toBe(
      "daemon · pid 40220 · The daemon restart failed.",
    );
  });

  it("draws the daemon's dot once, at the far right of the bar", async () => {
    const bar = await render(<StatusBar agent={AGENT} daemon={DAEMON} />);

    expect(bar.querySelectorAll(".status-bar-daemon .workspace-status-dot")).toHaveLength(1);
    expect(bar.querySelector(".workspace-status-bar")?.lastElementChild?.className).toBe(
      "status-bar-daemon",
    );
  });
});
