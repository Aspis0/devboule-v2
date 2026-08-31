// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Shell } from "./Shell";
import { useAppStore } from "../store/appStore";
import type { PluginEntry, PluginInventory } from "../types/ipc";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const READY: PluginEntry = {
  id: "polis",
  name: "Polis",
  version: "0.1.0",
  capabilities: ["oracle.search"],
  uiEntry: "ui/index.html",
  ready: true,
  reason: null,
};

const REFUSED: PluginEntry = {
  ...READY,
  ready: false,
  reason: "manifest digest mismatch",
};

function inventory(plugins: PluginEntry[], problem: string | null = null): PluginInventory {
  return { root: "C:/data/plugins", plugins, problem };
}

async function renderShell(): Promise<{
  container: HTMLDivElement;
  root: ReturnType<typeof createRoot>;
}> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      <Shell activeSurface="workspace">
        <div>Surface</div>
      </Shell>,
    );
  });
  return { container, root };
}

beforeEach(() => {
  useAppStore.setState({
    plugins: inventory([READY]),
    installing: null,
    refreshPlugins: vi.fn(async () => undefined),
  });
});

afterEach(() => {
  document.body.replaceChildren();
});

describe("Shell crescent", () => {
  it.each([
    ["open", inventory([READY]), null, 'aria-label="Open Polis"'],
    ["add", inventory([]), null, 'aria-label="Install Polis"'],
    ["installing", inventory([]), "polis", 'aria-label="Installing Polis"'],
    ["broken", inventory([REFUSED]), null, 'aria-label="Polis unavailable"'],
    ["unknown", null, null, 'aria-label="Polis status unknown"'],
  ] as const)(
    "gives the %s state an accessible name",
    async (_state, plugins, installing, name) => {
      useAppStore.setState({ plugins, installing });
      const { container, root } = await renderShell();
      expect(container.innerHTML).toContain(name);
      await act(async () => {
        root.unmount();
      });
    },
  );

  it("keeps the five crescent state classes distinct", async () => {
    const cases = [
      ["nav-point-open", inventory([READY]), null],
      ["nav-point-add", inventory([]), null],
      ["nav-point-installing", inventory([]), "polis"],
      ["nav-point-broken", inventory([REFUSED]), null],
      ["nav-point-unknown", null, null],
    ] as const;

    for (const [className, plugins, installing] of cases) {
      useAppStore.setState({ plugins, installing });
      const { container, root } = await renderShell();
      expect(container.innerHTML).toContain(className);
      await act(async () => {
        root.unmount();
      });
    }
  });

  it("closes on Escape even while an install is in flight", async () => {
    useAppStore.setState({ plugins: inventory([]), installing: "polis" });
    const { container, root } = await renderShell();

    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    const navigation = container.querySelector<HTMLElement>(".crescent-nav");
    if (sliver === null || navigation === null) throw new Error("crescent did not render");
    await act(async () => {
      sliver.focus();
    });
    expect(navigation.classList).toContain("crescent-nav-open");

    await act(async () => {
      sliver.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(navigation.classList).not.toContain("crescent-nav-open");
    await act(async () => {
      root.unmount();
    });
  });
});
