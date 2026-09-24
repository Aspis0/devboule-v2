// @vitest-environment happy-dom

import { act, type ReactElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ContextUsage, PlanUsage, SessionManifest } from "../../types/ipc";
import { recordPlanUsage } from "../../lib/planUsageStore";
import { ContextMeter, SessionContextMeter, type UsageSource } from "./ContextMeter";

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

function meter(props: Partial<Parameters<typeof ContextMeter>[0]>): ReactElement {
  return (
    <ContextMeter
      usage={props.usage ?? null}
      manifest={props.manifest ?? null}
      running={props.running ?? false}
    />
  );
}

async function openPopover(host: HTMLElement): Promise<HTMLElement> {
  const trigger = host.querySelector<HTMLButtonElement>(".workspace-context-meter-button");
  if (trigger === null) throw new Error("context meter did not render");
  await act(async () => trigger.click());
  const popover = document.querySelector<HTMLElement>(".workspace-context-popover");
  if (popover === null) throw new Error("context popover did not open");
  return popover;
}

describe("the composer's context meter", () => {
  it("renders nothing when there is no reading and nothing is running", async () => {
    const host = await render(meter({}));
    expect(host.querySelector(".workspace-context-meter-button")).toBeNull();
    expect(host.textContent).toBe("");
  });

  it("shows a track-only ring and no number while a turn runs without a reading", async () => {
    const host = await render(meter({ running: true }));
    const button = host.querySelector(".workspace-context-meter-button");
    if (button === null) throw new Error("pending meter did not render");
    expect(button.textContent).toBe("");
    // Track circle only — no accent arc claiming a value.
    expect(button.querySelectorAll("svg circle")).toHaveLength(1);
  });

  it("shows the percent and both token counts when both sides are known", async () => {
    const host = await render(meter({ usage: usage({ usedTokens: 76_000, maxTokens: 200_000 }) }));
    expect(host.querySelector(".workspace-context-meter-text")?.textContent).toBe(
      "38% · 76k / 200k",
    );
    // Track + arc.
    expect(host.querySelectorAll("svg circle")).toHaveLength(2);
  });

  it("shows one known number without a percent when the window is missing", async () => {
    // `used` is the only number a reading carries without its window: the
    // meter says that number and claims no percent.
    const host = await render(meter({ usage: usage({ usedTokens: 76_000 }) }));
    expect(host.textContent).toBe("76k");
    expect(host.textContent).not.toContain("%");
    expect(host.querySelectorAll("svg circle")).toHaveLength(1);
  });

  it("never pairs a reading with another model's window", async () => {
    const host = await render(
      meter({
        usage: usage({ modelId: "model-a", usedTokens: 76_000 }),
        manifest: manifest({
          currentModelId: "model-b",
          models: [{ modelId: "model-b", name: "B", contextTokens: 100_000 }],
        }),
      }),
    );
    expect(host.querySelector(".workspace-context-meter-button")).toBeNull();
    expect(host.textContent).toBe("");
  });
});

describe("the context popover", () => {
  it("opens above the meter with the reading and the last-turn note", async () => {
    const host = await render(
      meter({ usage: usage({ usedTokens: 76_000, maxTokens: 200_000, live: false }) }),
    );
    const popover = await openPopover(host);
    expect(popover.getAttribute("aria-label")).toBe("Context usage");
    expect(popover.querySelector(".workspace-context-percent")?.textContent).toBe("38% used");
    expect(popover.querySelector(".workspace-context-tokens")?.textContent).toBe(
      "76k / 200k tokens",
    );
    expect(popover.textContent).toContain("as of the last turn");
  });

  it("drops the last-turn note when the reading is live", async () => {
    const host = await render(
      meter({ usage: usage({ usedTokens: 76_000, maxTokens: 200_000, live: true }) }),
    );
    const popover = await openPopover(host);
    expect(popover.textContent).not.toContain("as of the last turn");
  });

  it("closes on Escape and on a click outside", async () => {
    const host = await render(meter({ usage: usage({ usedTokens: 76_000, maxTokens: 200_000 }) }));
    await openPopover(host);
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    expect(document.querySelector(".workspace-context-popover")).toBeNull();

    await openPopover(host);
    await act(async () => {
      document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    });
    expect(document.querySelector(".workspace-context-popover")).toBeNull();
  });

  it("says so plainly when no reading exists yet", async () => {
    const host = await render(meter({ running: true }));
    const popover = await openPopover(host);
    expect(popover.textContent).toContain("No context reading yet.");
  });

  it("shows the provider's plan windows and credits, spelled by the frame", async () => {
    const plan: PlanUsage = {
      type: "plan_usage",
      providerId: "codex-popover",
      planLabel: "plus",
      windows: [
        { durationMins: 300, usedPercent: 82 },
        { durationMins: 10_080, usedPercent: 39 },
      ],
      credits: { balance: "0", unlimited: false },
    };
    recordPlanUsage(plan);
    const host = await render(
      meter({
        usage: usage({ usedTokens: 76_000, maxTokens: 200_000 }),
        manifest: manifest({ providerId: "codex-popover", models: [] }),
      }),
    );
    const popover = await openPopover(host);
    expect(popover.querySelector(".workspace-context-plan-chip")?.textContent).toBe("plus");
    const labels = [...popover.querySelectorAll(".workspace-context-window-label")].map(
      (node) => node.textContent,
    );
    expect(labels).toEqual(["5-hour", "Weekly"]);
    expect(popover.querySelectorAll(".workspace-context-window-fill")).toHaveLength(2);
    expect(popover.textContent).toContain("Credits: 0");
  });

  it("omits a percent the frame never sent instead of showing zero", async () => {
    recordPlanUsage({
      type: "plan_usage",
      providerId: "codex-popover-partial",
      windows: [{ durationMins: 300 }],
    });
    const host = await render(
      meter({
        usage: usage({ usedTokens: 76_000, maxTokens: 200_000 }),
        manifest: manifest({ providerId: "codex-popover-partial", models: [] }),
      }),
    );
    const popover = await openPopover(host);
    const row = popover.querySelector(".workspace-context-window");
    expect(row?.textContent).toContain("5-hour");
    expect(row?.textContent).not.toContain("%");
    // An empty track: no fill claims a value.
    expect(row?.querySelectorAll(".workspace-context-window-fill")).toHaveLength(0);
    expect(popover.textContent).not.toContain("Credits");
  });

  it("says the provider reports no plan usage when no frame ever arrived", async () => {
    const host = await render(
      meter({
        usage: usage({ usedTokens: 76_000, maxTokens: 200_000 }),
        manifest: manifest({ providerId: "provider-without-plan", models: [] }),
      }),
    );
    const popover = await openPopover(host);
    expect(popover.textContent).toContain("This provider does not report plan usage.");
  });

  it("keeps the countdown moving while the popover sits open", async () => {
    // F10: the copy is computed from a clock the popover owns; without the
    // timer it would freeze at whatever "resets in" said when it opened.
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
    try {
      const nowSeconds = Math.floor(Date.now() / 1000);
      recordPlanUsage({
        type: "plan_usage",
        providerId: "codex-countdown",
        windows: [{ durationMins: 300, usedPercent: 82, resetsAt: nowSeconds + 61 * 60 }],
      });
      const host = await render(
        meter({
          usage: usage({ usedTokens: 76_000, maxTokens: 200_000 }),
          manifest: manifest({ providerId: "codex-countdown", models: [] }),
        }),
      );
      const popover = await openPopover(host);
      expect(popover.textContent).toContain("resets in 1 h");
      await act(async () => {
        vi.advanceTimersByTime(60 * 60_000);
      });
      expect(popover.textContent).toContain("resets in 1 min");
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("binding the meter to its session", () => {
  it("re-renders from the session's usage lane alone", async () => {
    // The surface re-renders on `subscribe`; this component must move on
    // `subscribeUsage` — the two lanes are the whole F5 fix.
    let notifyUsage: (() => void) | null = null;
    let stored: ContextUsage | null = null;
    const source: UsageSource = {
      subscribeUsage(listener) {
        notifyUsage = listener;
        return () => {
          notifyUsage = null;
        };
      },
      getContextUsage: () => stored,
    };
    const host = await render(
      <SessionContextMeter session={source} manifest={null} running={false} />,
    );
    // No reading, nothing running: nothing at all.
    expect(host.querySelector(".workspace-context-meter-button")).toBeNull();
    await act(async () => {
      stored = { type: "context_usage", usedTokens: 76_000, maxTokens: 200_000, live: true };
      notifyUsage?.();
    });
    expect(host.querySelector(".workspace-context-meter-text")?.textContent).toBe(
      "38% · 76k / 200k",
    );
  });
});
