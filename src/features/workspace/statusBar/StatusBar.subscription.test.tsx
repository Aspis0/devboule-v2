// @vitest-environment happy-dom
import { Profiler, act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { recordPlanUsage } from "../../../lib/planUsageStore";
import type { DaemonStatus, PlanUsage } from "../../../types/ipc";
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

function frame(providerId: string, usedPercent: number): PlanUsage {
  return {
    type: "plan_usage",
    providerId,
    windows: [{ durationMins: 300, usedPercent, resetsAt: later }],
  };
}

let root: Root | null = null;
let host: HTMLDivElement | null = null;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  host?.remove();
  root = null;
  host = null;
});

describe("the status bar's usage subscription", () => {
  it("does not commit for a usage frame from a provider that is not in front", async () => {
    recordPlanUsage(frame("claude", 10));
    const commits = vi.fn();
    host = document.createElement("div");
    document.body.append(host);
    root = createRoot(host);
    await act(async () =>
      root?.render(
        <Profiler id="status-bar" onRender={commits}>
          <StatusBar
            agent={{
              sessionId: "bar-sub",
              title: "Focused",
              attentionWord: null,
              attentionTone: null,
              working: false,
              stateWord: null,
              providerId: "claude",
            }}
            daemon={DAEMON}
          />
        </Profiler>,
      ),
    );
    commits.mockClear();

    await act(async () => recordPlanUsage(frame("codex", 55)));
    expect(commits).not.toHaveBeenCalled();

    await act(async () => recordPlanUsage(frame("claude", 20)));
    expect(commits).toHaveBeenCalled();
  });
});
