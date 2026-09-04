// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { useAppStore } from "../../store/appStore";
import type { PluginEntry, PluginInventory } from "../../types/ipc";
import { FREE_SKILL, MOCK_MARKETPLACE_ENTRIES } from "./mockData";
import { MarketplaceSurface, skillIsInstallable } from "./MarketplaceSurface";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const READY_POLIS: PluginEntry = {
  id: "polis",
  name: "Polis",
  version: "0.1.0",
  capabilities: ["oracle.search"],
  uiEntry: "ui/index.html",
  ready: true,
  reason: null,
};

function inventory(plugins: PluginEntry[]): PluginInventory {
  return { root: "C:/data/plugins", plugins, problem: null };
}

async function renderMarketplace(): Promise<{
  container: HTMLDivElement;
  root: ReturnType<typeof createRoot>;
}> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<MarketplaceSurface />);
  });
  return { container, root };
}

beforeEach(() => {
  useAppStore.setState({ installedSkills: [], plugins: null });
});

afterEach(() => {
  document.body.replaceChildren();
});

describe("MarketplaceSurface", () => {
  it("renders plugin, pack, and skill entries", async () => {
    const { container, root } = await renderMarketplace();

    for (const kind of ["plugin", "pack", "skill"]) {
      expect(container.querySelector(`[data-kind="${kind}"]`)).not.toBeNull();
    }

    await act(async () => root.unmount());
  });

  it("does not call an unchecked plugin absent", async () => {
    const { container, root } = await renderMarketplace();
    const polis = container.querySelector<HTMLElement>('[data-entry-id="polis"]');
    if (polis === null) throw new Error("Polis row missing");

    expect(polis.textContent).not.toContain("Install from the crescent +.");
    expect(polis.textContent).toContain("Checking plugin status.");
    await act(async () => root.unmount());
  });

  it("labels the catalog as a fixture and explains session-only Workspace skills", async () => {
    const { container, root } = await renderMarketplace();

    const headerNote = container.querySelector(".marketplace-header-note")?.textContent ?? "";
    expect(headerNote).toContain("Workspace");
    expect(headerNote).toMatch(/not saved to disk/i);
    expect(container.textContent).toContain("Demo catalog — fixtures, not a live store.");
    await act(async () => root.unmount());
  });

  it("reports that a free skill is in Workspace for this session only", async () => {
    const { container, root } = await renderMarketplace();
    const install = container.querySelector<HTMLButtonElement>(
      `[data-entry-id="${FREE_SKILL.id}"] [data-action="install-skill"]`,
    );
    if (install === null) throw new Error("free skill Install control missing");

    await act(async () => install.click());

    expect(container.querySelector('[role="alert"]')?.textContent).toMatch(
      /Workspace.*session-only|Workspace.*session only/i,
    );
    expect(container.querySelector('[role="alert"]')?.textContent).toMatch(/not saved to disk/i);
    await act(async () => root.unmount());
  });

  it("keeps Polis free and does not offer a Buy control", async () => {
    useAppStore.setState({ plugins: inventory([]) });
    const { container, root } = await renderMarketplace();
    const polis = container.querySelector<HTMLElement>('[data-entry-id="polis"]');
    if (polis === null) throw new Error("Polis row missing");

    expect(polis.querySelector('[data-action="buy"]')).toBeNull();
    expect(polis.querySelector(".marketplace-price")?.textContent).not.toBe("$12");
    expect(polis.textContent).toContain("Install from the crescent +.");
    await act(async () => root.unmount());
  });

  it("shows Polis as Installed only when the plugin inventory says it is ready", async () => {
    useAppStore.setState({ plugins: inventory([READY_POLIS]) });
    const { container, root } = await renderMarketplace();
    const polis = container.querySelector<HTMLElement>('[data-entry-id="polis"]');
    if (polis === null) throw new Error("Polis row missing");

    expect(polis.textContent).toContain("Installed");
    await act(async () => root.unmount());
  });

  it("uses install controls only for free skills", () => {
    const paidSkill = { ...FREE_SKILL, price: "$5" };

    expect(skillIsInstallable(paidSkill)).toBe(false);
    expect(skillIsInstallable(FREE_SKILL)).toBe(true);
    expect(skillIsInstallable({ ...FREE_SKILL, kind: "plugin" })).toBe(false);
  });

  it("uses pressed filters rather than a tab pattern", async () => {
    const { container, root } = await renderMarketplace();

    expect(container.querySelector('[role="tablist"]')).toBeNull();
    expect(container.querySelector('[role="tab"]')).toBeNull();
    expect(container.querySelectorAll(".marketplace-filter")).toHaveLength(4);
    for (const filter of container.querySelectorAll<HTMLButtonElement>(".marketplace-filter")) {
      expect(filter.getAttribute("aria-pressed")).toBe(
        filter.classList.contains("marketplace-filter-active") ? "true" : "false",
      );
    }
    expect(
      container
        .querySelector<HTMLButtonElement>(".marketplace-filter-active")
        ?.getAttribute("aria-pressed"),
    ).toBe("true");
    await act(async () => root.unmount());
  });

  it("shows the not-wired alert when buying a pack", async () => {
    const { container, root } = await renderMarketplace();
    const pack = MOCK_MARKETPLACE_ENTRIES.find((entry) => entry.kind === "pack");
    if (pack === undefined) throw new Error("pack fixture missing");
    const buy = container.querySelector<HTMLButtonElement>(
      `[data-entry-id="${pack.id}"] [data-action="buy"]`,
    );
    if (buy === null) throw new Error("pack Buy control missing");

    await act(async () => buy.click());

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(
      "Purchases are not wired yet.",
    );
    await act(async () => root.unmount());
  });

  it("adds a free skill to the installed skills list", async () => {
    const { container, root } = await renderMarketplace();
    const install = container.querySelector<HTMLButtonElement>(
      `[data-entry-id="${FREE_SKILL.id}"] [data-action="install-skill"]`,
    );
    if (install === null) throw new Error("free skill Install control missing");

    await act(async () => install.click());

    expect(useAppStore.getState().installedSkills).toEqual([
      {
        id: FREE_SKILL.id,
        name: FREE_SKILL.name,
        author: FREE_SKILL.author,
        description: FREE_SKILL.description,
      },
    ]);
    await act(async () => root.unmount());
  });
});
