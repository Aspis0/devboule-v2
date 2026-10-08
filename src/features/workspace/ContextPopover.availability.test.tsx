// @vitest-environment happy-dom

import { act, type ReactElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentFinished } from "../../lib/agentSession";
import { recordPlanUsage } from "../../lib/planUsageStore";
import type { ContextUsage, PlanUsage, SessionManifest } from "../../types/ipc";
import { ContextMeter } from "./ContextMeter";

let container: HTMLDivElement | null = null;
let unmount: (() => Promise<void>) | null = null;

async function render(element: ReactElement): Promise<HTMLElement> {
  const host = document.createElement("div");
  document.body.appendChild(host);
  container = host;
  const root = createRoot(host);
  unmount = () => act(async () => root.unmount());
  await act(async () => root.render(element));
  return host;
}

afterEach(async () => {
  await unmount?.();
  unmount = null;
  container?.remove();
  container = null;
});

function usage(partial: Partial<ContextUsage>): ContextUsage {
  return { type: "context_usage", usedTokens: 0, live: true, ...partial };
}

function manifest(
  partial: Partial<SessionManifest> & { models: SessionManifest["models"] },
): SessionManifest {
  return { type: "session_manifest", ...partial };
}

function finishedTurn(costUsd: number | undefined): AgentFinished {
  return {
    stopReason: "stop",
    ...(costUsd === undefined ? {} : { usage: { costUsd } }),
  };
}

function plan(partial: Partial<PlanUsage> & { providerId: string }): PlanUsage {
  return { type: "plan_usage", windows: [], ...partial };
}

async function openPopover(host: HTMLElement): Promise<HTMLElement> {
  const trigger = host.querySelector<HTMLButtonElement>(".workspace-context-meter-button");
  if (trigger === null) throw new Error("context meter did not render");
  await act(async () => trigger.click());
  const popover = document.querySelector<HTMLElement>(".workspace-context-popover");
  if (popover === null) throw new Error("context popover did not open");
  return popover;
}

/** Render the meter with a reading (so the popover can open) and read its rows. */
async function popoverRows(props: {
  providerId?: string;
  currentModelProviderId?: string;
  lastFinished?: AgentFinished | null;
}): Promise<HTMLElement> {
  const host = await render(
    <ContextMeter
      usage={usage({ usedTokens: 76_000, maxTokens: 200_000 })}
      manifest={manifest({
        ...(props.providerId === undefined ? {} : { providerId: props.providerId }),
        ...(props.currentModelProviderId === undefined
          ? {}
          : { currentModelProviderId: props.currentModelProviderId }),
        models: [],
      })}
      lastFinished={props.lastFinished ?? null}
    />,
  );
  return openPopover(host);
}

describe("the popover's per-provider usage rows", () => {
  it("shows Claude's cost value and its plan windows, with no age for a frame first seen here", async () => {
    recordPlanUsage(
      plan({
        providerId: "claude",
        windows: [{ durationMins: 300, usedPercent: 33, resetsAt: 1_790_632_800 }],
      }),
    );
    const popover = await popoverRows({
      providerId: "claude",
      lastFinished: finishedTurn(1.239),
    });
    expect(popover.textContent).toContain("Turn cost: $1.23");
    expect(popover.textContent).toContain("5-hour");
    expect(popover.textContent).not.toContain("updated");
    expect(popover.textContent).not.toContain("$0.00");
  });

  it("dates a frame from the moment it changed, and says nothing when the clock ran backwards", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(new Date("2026-10-02T12:00:00Z"));
      recordPlanUsage(
        plan({
          providerId: "claude",
          windows: [{ durationMins: 300, usedPercent: 34, resetsAt: 1_790_632_800 }],
        }),
      );
      const fresh = await popoverRows({ providerId: "claude", lastFinished: null });
      expect(fresh.textContent).toContain("updated just now");
      await unmount?.();
      container?.remove();

      vi.setSystemTime(new Date("2026-10-02T11:00:00Z"));
      const rolledBack = await popoverRows({ providerId: "claude", lastFinished: null });
      expect(rolledBack.textContent).toContain("34%");
      expect(rolledBack.textContent).not.toContain("updated");
    } finally {
      vi.useRealTimers();
    }
  });

  it("tells Codex's cost apart from Codex's plan reading", async () => {
    // Codex frames name no turn cost and this session has no rate-limit
    // frame yet: two different absences, never one silence.
    const popover = await popoverRows({
      providerId: "codex",
      lastFinished: finishedTurn(undefined),
    });
    expect(popover.textContent).toContain("Turn cost: not reported by this agent");
    expect(popover.textContent).not.toContain("codex");
    expect(popover.textContent).toContain("No plan reading yet.");
    expect(popover.textContent).not.toContain("$0.00");
  });

  it("tells pi's sendable cost apart from pi's plan limits", async () => {
    const popover = await popoverRows({ providerId: "pi", lastFinished: null });
    expect(popover.textContent).toContain("Turn cost: no reading yet");
    expect(popover.textContent).toContain("This provider does not report plan limits.");
  });

  it("claims no cost absence for an ACP provider whose adapter drops cost, but no plan limits", async () => {
    const popover = await popoverRows({
      providerId: "gemini",
      lastFinished: finishedTurn(undefined),
    });
    expect(popover.textContent).toContain("Turn cost: no reading yet");
    expect(popover.textContent).not.toContain("gemini");
    expect(popover.textContent).toContain("This provider does not report plan limits.");
    expect(popover.textContent).not.toContain("$0.00");
  });

  it("shows the OpenCode Go plan under a Pi session on an OpenCode model, beside pi's own turn cost", async () => {
    // A far-future reset keeps both windows current whatever day the suite runs.
    recordPlanUsage(
      plan({
        providerId: "opencode-go",
        planLabel: "OpenCode Go",
        windows: [
          { durationMins: 300, usedPercent: 58, resetsAt: 2_000_000_000 },
          { durationMins: 10_080, usedPercent: 41, resetsAt: 2_000_000_000 },
        ],
      }),
    );
    const popover = await popoverRows({
      providerId: "pi",
      currentModelProviderId: "opencode",
      lastFinished: null,
    });
    expect(popover.textContent).toContain("5-hour");
    expect(popover.textContent).toContain("Weekly");
    expect(popover.textContent).toContain("58%");
    expect(popover.textContent).toContain("41%");
    expect(popover.textContent).toContain("Turn cost: no reading yet");
    expect(popover.textContent).not.toContain("does not report plan limits");
  });

  it("keeps the absence line for a Pi backend that reports no plan, even with the Go frame cached", async () => {
    recordPlanUsage(
      plan({
        providerId: "opencode-go",
        planLabel: "OpenCode Go",
        windows: [{ durationMins: 300, usedPercent: 58, resetsAt: 2_000_000_000 }],
      }),
    );
    const popover = await popoverRows({
      providerId: "pi",
      currentModelProviderId: "anthropic",
      lastFinished: null,
    });
    expect(popover.textContent).toContain("This provider does not report plan limits.");
    expect(popover.textContent).not.toContain("58%");
  });

  it("claims nothing about a manifest that named no provider id", async () => {
    const popover = await popoverRows({ lastFinished: null });
    expect(popover.textContent).toContain("Turn cost: no reading yet");
    expect(popover.textContent).toContain("No plan reading yet.");
    expect(popover.textContent).not.toContain("not reported by");
    expect(popover.textContent).not.toContain("does not report");
  });

  it("labels a plan frame's age and a window whose reset already passed", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      vi.setSystemTime(new Date("2026-10-02T11:00:00Z"));
      recordPlanUsage(
        plan({
          providerId: "codex",
          windows: [{ durationMins: 300, usedPercent: 70, resetsAt: 1_790_632_800 }],
        }),
      );
      vi.setSystemTime(new Date("2026-10-02T12:00:00Z"));
      recordPlanUsage(
        plan({
          providerId: "codex",
          windows: [
            {
              durationMins: 300,
              usedPercent: 82,
              resetsAt: Math.floor(Date.now() / 1000) - 3_600,
            },
          ],
        }),
      );
      vi.setSystemTime(new Date("2026-10-02T13:00:00Z"));
      const popover = await popoverRows({ providerId: "codex", lastFinished: null });
      expect(popover.textContent).toContain("updated 1 h ago");
      expect(popover.textContent).toContain("resets now");
      expect(popover.textContent).not.toContain("resets in");
    } finally {
      vi.useRealTimers();
    }
  });
});
