// @vitest-environment happy-dom

import { act, type ReactElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ContextUsage, PlanUsage, SessionManifest } from "../../types/ipc";
import { recordPlanUsage } from "../../lib/planUsageStore";
import { ContextMeter, SessionContextMeter, type UsageSource } from "./ContextMeter";
import { placeContextPopover } from "./ContextPopover";

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

/** The geometry happy-dom cannot compute (no layout): a box at given edges. */
function rect(left: number, top: number, width: number, height: number): DOMRect {
  return {
    left,
    top,
    width,
    height,
    right: left + width,
    bottom: top + height,
    x: left,
    y: top,
    toJSON: () => ({ left, top, width, height, right: left + width, bottom: top + height }),
  };
}

/** Pin `window.innerWidth` (happy-dom's viewport) for one test. */
function setViewportWidth(width: number): () => void {
  const original = Object.getOwnPropertyDescriptor(window, "innerWidth");
  Object.defineProperty(window, "innerWidth", { value: width, configurable: true });
  return () => {
    if (original !== undefined) Object.defineProperty(window, "innerWidth", original);
    else delete (window as { innerWidth?: number }).innerWidth;
  };
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

  it("draws the panel from the body, outside the pane that would clip it", async () => {
    // The live check: `.workspace-center-panel` has `overflow: hidden` and
    // cut the old in-pane popover at 974 px, losing its right edge and the
    // "resets in …" labels. A body child cannot be clipped by that pane.
    const host = await render(meter({ usage: usage({ usedTokens: 76_000, maxTokens: 200_000 }) }));
    const popover = await openPopover(host);
    expect(popover.parentElement).toBe(document.body);
    expect(popover.closest(".workspace-context-meter")).toBeNull();
    expect(host.querySelector(".workspace-context-popover")).toBeNull();
  });

  it("keeps the panel inside a narrow viewport at the right edge", async () => {
    const host = await render(meter({ usage: usage({ usedTokens: 76_000, maxTokens: 200_000 }) }));
    const trigger = host.querySelector<HTMLButtonElement>(".workspace-context-meter-button");
    if (trigger === null) throw new Error("context meter did not render");
    // Live geometry, injected: the meter at 380..394 of a 400 px viewport —
    // centring a 300 px panel would span 237..537, past the right edge.
    trigger.getBoundingClientRect = () => rect(380, 500, 14, 14);
    const restoreWidth = setViewportWidth(400);
    try {
      const popover = await openPopover(host);
      expect(popover.style.position).toBe("fixed");
      const left = Number.parseFloat(popover.style.left);
      // 400 − 8 margin − 300 width = 92; centre would be 237.
      expect(left).toBe(92);
      expect(left + 300).toBeLessThanOrEqual(400 - 8);
    } finally {
      restoreWidth();
    }
  });

  it("follows the meter when the composer moves under it", async () => {
    const host = await render(meter({ usage: usage({ usedTokens: 76_000, maxTokens: 200_000 }) }));
    const trigger = host.querySelector<HTMLButtonElement>(".workspace-context-meter-button");
    if (trigger === null) throw new Error("context meter did not render");
    trigger.getBoundingClientRect = () => rect(300, 500, 14, 14);
    const restoreWidth = setViewportWidth(1024);
    try {
      const popover = await openPopover(host);
      // Centre 307 − 150 = 157, inside [8, 716].
      expect(popover.style.left).toBe("157px");
      trigger.getBoundingClientRect = () => rect(100, 300, 14, 14);
      await act(async () => {
        window.dispatchEvent(new Event("resize"));
      });
      // Centre 107 − 150 = −43, clamped to the 8 px margin.
      expect(popover.style.left).toBe("8px");
    } finally {
      restoreWidth();
    }
  });
});

describe("placeContextPopover", () => {
  const VIEWPORT = { width: 1024, height: 768 };

  it("centres the panel above its anchor with the gap", () => {
    // Anchor centre 507 − 150 = 357; top 400 − 8 gap − 160 height = 232.
    expect(
      placeContextPopover(
        { left: 500, right: 514, top: 400, bottom: 414 },
        { width: 300, height: 160 },
        VIEWPORT,
        8,
      ),
    ).toEqual({ left: 357, top: 232, width: 300, above: true });
  });

  it("flips below when the top has no room", () => {
    const placement = placeContextPopover(
      { left: 100, right: 114, top: 30, bottom: 44 },
      { width: 300, height: 160 },
      VIEWPORT,
      8,
    );
    expect(placement.above).toBe(false);
    expect(placement.top).toBe(52); // anchor.bottom 44 + gap 8
  });

  it("clamps a panel wider than the viewport to the margins", () => {
    const placement = placeContextPopover(
      { left: 170, right: 184, top: 400, bottom: 414 },
      { width: 400, height: 160 },
      { width: 360, height: 768 },
      8,
    );
    expect(placement.width).toBe(344); // 360 − 2 × 8 margin
    expect(placement.left).toBe(8);
    expect(placement.left + placement.width).toBeLessThanOrEqual(360 - 8);
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
