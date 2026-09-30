// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/tauri")>();
  return {
    ...actual,
    providersList: vi.fn(),
  };
});

import { providersList } from "../../lib/tauri";
import { recordPlanUsage } from "../../lib/planUsageStore";
import type { PlanUsage, ProviderCatalog, ProviderInfo } from "../../types/ipc";
import { UsagePanel } from "./UsagePanel";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function catalog(providers: ProviderInfo[], unreadableDirs = 0): ProviderCatalog {
  return { providers, unreadableDirs };
}

function provider(overrides: Partial<ProviderInfo> & { id: string }): ProviderInfo {
  return {
    executable: `${overrides.id}-bin`,
    acpAvailable: true,
    authentication: "unknown",
    ...overrides,
  };
}

function plan(partial: Partial<PlanUsage> & { providerId: string }): PlanUsage {
  return { type: "plan_usage", windows: [], ...partial };
}

describe("the Usage settings page", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    // Wipe implementations, not just calls: a rejected listing in one test
    // must not outlive it into the next.
    vi.mocked(providersList).mockReset();
  });

  afterEach(async () => {
    if (root !== undefined) await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  function section(providerId: string): HTMLElement | null {
    return container.querySelector(`section[aria-label='Plan usage: ${providerId}']`);
  }

  async function renderPanel() {
    root = createRoot(container);
    await act(async () => root.render(<UsagePanel />));
    await act(async () => undefined);
  }

  it("shows a status line while the provider list is in flight", async () => {
    vi.mocked(providersList).mockImplementation(
      () => new Promise<ProviderCatalog>(() => undefined),
    );
    await renderPanel();
    expect(container.textContent).toContain("Listing the configured providers…");
  });

  it("shows each provider's honest no-reading line", async () => {
    vi.mocked(providersList).mockResolvedValue(
      catalog([
        provider({ id: "claude", protocol: "stream-json" }),
        provider({ id: "grok", protocol: "acp" }),
        provider({ id: "codex", protocol: "codex-app-server", enabled: false }),
        provider({ id: "ghost", protocol: "stream-json", installed: false }),
      ]),
    );
    await renderPanel();

    expect(section("claude")?.textContent).toContain(
      "No plan reading yet — one appears here when a session of this provider sends it.",
    );
    // A provider the daemon cannot produce plan usage for says so, instead of
    // promising a reading that can never arrive.
    expect(section("grok")?.textContent).toContain("This provider does not report plan usage.");
    expect(section("grok")?.textContent).not.toContain("No plan reading yet");
    // A switched-off provider cannot start the session that would send one.
    expect(section("codex")?.textContent).toContain(
      "the provider is switched off, so no session can send one",
    );
    // A provider the catalogue only offers (not installed) has no plan to read.
    expect(section("ghost")).toBeNull();
  });

  it("takes a missing protocol as unknown, never as cannot-report", async () => {
    // An older daemon sends no protocol field at all; the page then claims
    // nothing about what the provider can report.
    vi.mocked(providersList).mockResolvedValue(
      catalog([
        provider({ id: "claude" }),
        provider({ id: "grok", protocol: "acp" }),
        provider({ id: "grok-off", enabled: false }),
      ]),
    );
    await renderPanel();

    expect(section("claude")?.textContent).toContain("No plan reading yet");
    expect(section("claude")?.textContent).not.toContain("does not report");
    // A present protocol that is neither reporting road still says so.
    expect(section("grok")?.textContent).toContain("does not report");
    // Switched off with no protocol answer, the true line is the
    // switched-off one: no session can send a reading.
    expect(section("grok-off")?.textContent).toContain(
      "the provider is switched off, so no session can send one",
    );
    expect(section("grok-off")?.textContent).not.toContain("does not report");
  });

  it("shows windows, percent, reset and credits from a plan reading", async () => {
    recordPlanUsage(
      plan({
        providerId: "usage-live",
        planLabel: "Pro",
        windows: [{ durationMins: 300, usedPercent: 33, resetsAt: 1_000 }],
        credits: { balance: "$1.25" },
      }),
    );
    vi.mocked(providersList).mockResolvedValue(
      catalog([provider({ id: "usage-live", protocol: "stream-json" })]),
    );
    await renderPanel();

    const group = section("usage-live");
    expect(group?.textContent).toContain("Pro");
    expect(group?.textContent).toContain("5-hour");
    expect(group?.textContent).toContain("33%");
    // resetsAt 1s is far in the past: the label says so, never a negative.
    expect(group?.textContent).toContain("resets now");
    expect(group?.textContent).toContain("$1.25");
    expect(group?.querySelector(".plan-window-fill")).not.toBeNull();
  });

  it("shows only the fields a partial reading carried", async () => {
    recordPlanUsage(
      plan({
        providerId: "usage-partial",
        windows: [{ durationMins: 10_080 }],
      }),
    );
    vi.mocked(providersList).mockResolvedValue(
      catalog([provider({ id: "usage-partial", protocol: "codex-app-server" })]),
    );
    await renderPanel();

    const group = section("usage-partial");
    expect(group?.textContent).toContain("Weekly");
    expect(group?.textContent).not.toContain("%");
    expect(group?.textContent).not.toContain("Credits");
    expect(group?.querySelector(".plan-window-fill")).toBeNull();
    // A reading that arrived, however thin, is not "no reading".
    expect(group?.textContent).not.toContain("No plan reading yet");
  });

  it("shows the no-reading line, not a blank group, for a frame that carried nothing", async () => {
    recordPlanUsage(plan({ providerId: "usage-blank" }));
    vi.mocked(providersList).mockResolvedValue(
      catalog([provider({ id: "usage-blank", protocol: "codex-app-server" })]),
    );
    await renderPanel();

    const group = section("usage-blank");
    expect(group?.textContent).toContain("No plan reading yet");
    expect(group?.querySelector(".settings-stack")).toBeNull();
  });

  it("keeps the text true and clamps the bar for an overage percent", async () => {
    recordPlanUsage(
      plan({
        providerId: "usage-overage",
        windows: [{ durationMins: 300, usedPercent: 130 }],
      }),
    );
    vi.mocked(providersList).mockResolvedValue(
      catalog([provider({ id: "usage-overage", protocol: "stream-json" })]),
    );
    await renderPanel();

    const group = section("usage-overage");
    expect(group?.textContent).toContain("130%");
    const fill = group?.querySelector<HTMLElement>(".plan-window-fill");
    expect(fill?.style.width).toBe("100%");
  });

  it("counts a future reset down from the frame's reset time", async () => {
    recordPlanUsage(
      plan({
        providerId: "usage-future",
        windows: [
          { durationMins: 300, usedPercent: 10, resetsAt: Math.floor(Date.now() / 1000) + 90 * 60 },
        ],
      }),
    );
    vi.mocked(providersList).mockResolvedValue(
      catalog([provider({ id: "usage-future", protocol: "stream-json" })]),
    );
    await renderPanel();

    expect(section("usage-future")?.textContent).toContain("resets in 1 h");
  });

  it("renders two windows of one length as two rows", async () => {
    recordPlanUsage(
      plan({
        providerId: "usage-twins",
        windows: [
          { durationMins: 300, usedPercent: 11 },
          { durationMins: 300, usedPercent: 22 },
        ],
      }),
    );
    vi.mocked(providersList).mockResolvedValue(
      catalog([provider({ id: "usage-twins", protocol: "stream-json" })]),
    );
    await renderPanel();

    const group = section("usage-twins");
    expect(group?.querySelectorAll(".plan-window")).toHaveLength(2);
    expect(group?.textContent).toContain("11%");
    expect(group?.textContent).toContain("22%");
  });

  it("swaps the empty state for a reading that lands while the page is up", async () => {
    vi.mocked(providersList).mockResolvedValue(
      catalog([provider({ id: "usage-arrives", protocol: "stream-json" })]),
    );
    await renderPanel();
    expect(section("usage-arrives")?.textContent).toContain("No plan reading yet");

    await act(async () => {
      recordPlanUsage(
        plan({
          providerId: "usage-arrives",
          windows: [{ durationMins: 300, usedPercent: 10, resetsAt: 1_000 }],
        }),
      );
    });
    expect(section("usage-arrives")?.textContent).toContain("10%");
    expect(section("usage-arrives")?.textContent).not.toContain("No plan reading yet");
  });

  it("handles an unreadable PATH directory like the Providers page does", async () => {
    vi.mocked(providersList).mockResolvedValue(catalog([], 2));
    await renderPanel();
    expect(container.textContent).toContain(
      "No agent CLI found, but 2 PATH directories could not be read",
    );

    await act(async () => root.unmount());
    vi.mocked(providersList).mockResolvedValue(catalog([]));
    root = createRoot(container);
    await act(async () => root.render(<UsagePanel />));
    await act(async () => undefined);
    expect(container.textContent).toContain("No agent CLI found on PATH");
  });

  it("reports a failed listing as an alert, never as an empty page", async () => {
    vi.mocked(providersList).mockRejectedValue(new Error("daemon unreachable"));
    await renderPanel();

    expect(container.querySelector("[role='alert']")?.textContent).toContain("daemon unreachable");
    expect(container.textContent).not.toContain("No agent CLI found");
    expect(container.querySelector("[role='status']")).toBeNull();
  });

  it("retries a failed listing from the error state", async () => {
    vi.mocked(providersList).mockRejectedValueOnce(new Error("daemon unreachable"));
    vi.mocked(providersList).mockResolvedValue(
      catalog([provider({ id: "usage-retried", protocol: "stream-json" })]),
    );
    await renderPanel();
    expect(container.querySelector("[role='alert']")).not.toBeNull();

    const retry = container.querySelector<HTMLButtonElement>("[role='alert'] button");
    if (!retry) throw new Error("Retry button did not render");
    await act(async () => retry.click());
    await act(async () => undefined);

    expect(container.querySelector("[role='alert']")).toBeNull();
    expect(section("usage-retried")).not.toBeNull();
  });
});
