// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../features/plugins/install", () => ({
  chooseAndInstall: vi.fn(),
}));

import { chooseAndInstall } from "../features/plugins/install";
import { useAppStore } from "../store/appStore";
import type { PluginEntry, PluginInventory } from "../types/ipc";
import { Shell } from "./Shell";

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
  maxPayloadBytes: 16 * 1024 * 1024,
  payloadBudgetClamped: false,
};

const REFUSED: PluginEntry = {
  ...READY,
  ready: false,
  reason: "manifest digest mismatch",
  maxPayloadBytes: null,
  payloadBudgetClamped: null,
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
    installError: null,
    plugins: inventory([READY]),
    installing: null,
    selectSurface: vi.fn(),
    refreshPlugins: vi.fn(async () => undefined),
  });
});

afterEach(() => {
  document.body.replaceChildren();
  vi.clearAllMocks();
});

describe("Shell crescent", () => {
  it("renders Marketplace as an accessible crescent point", async () => {
    const { container, root } = await renderShell();

    expect(container.innerHTML).toContain('aria-label="Open Marketplace"');
    await act(async () => root.unmount());
  });

  it("does not render paging arrows when all six product surfaces fit", async () => {
    const { container, root } = await renderShell();

    expect(container.querySelector(".crescent-page-arrow-prev")).toBeNull();
    expect(container.querySelector(".crescent-page-arrow-next")).toBeNull();
    await act(async () => root.unmount());
  });

  it("uses the crescent plus to choose a Polis folder without selecting the surface", async () => {
    const selectSurface = vi.fn();
    useAppStore.setState({
      plugins: inventory([]),
      selectSurface,
    });
    vi.mocked(chooseAndInstall).mockResolvedValue(true);
    const { container, root } = await renderShell();

    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    if (sliver === null) throw new Error("crescent did not render");
    await act(async () => sliver.focus());

    const polis = container.querySelector<HTMLButtonElement>('[aria-label="Install Polis"]');
    if (polis === null) throw new Error("Polis install point did not render");
    await act(async () => polis.click());

    expect(chooseAndInstall).toHaveBeenCalledWith("polis", "Polis");
    expect(selectSurface).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("shows and dismisses a crescent install error while the nav is open", async () => {
    useAppStore.setState({
      installError: "the folder was refused",
    });
    const { container, root } = await renderShell();

    expect(container.querySelector('[role="alert"]')).toBeNull();

    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    if (sliver === null) throw new Error("crescent did not render");
    await act(async () => sliver.focus());

    const alert = container.querySelector<HTMLElement>('[role="alert"]');
    expect(alert?.textContent).toContain(
      "The last install did not happen — the folder was refused",
    );
    const dismiss = alert?.querySelector<HTMLButtonElement>("button");
    if (dismiss === undefined || dismiss === null) throw new Error("dismiss control missing");
    await act(async () => dismiss.click());
    expect(container.querySelector('[role="alert"]')).toBeNull();
    await act(async () => root.unmount());
  });

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

  it("a click on the line opens the nav when no hover preceded it", async () => {
    const { container, root } = await renderShell();

    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    const navigation = container.querySelector<HTMLElement>(".crescent-nav");
    if (sliver === null || navigation === null) throw new Error("crescent did not render");
    // A tap or a click with the pointer arriving too fast: no pointerenter,
    // no focus — the click itself must open the nav.
    await act(async () => {
      sliver.click();
    });
    expect(navigation.classList).toContain("crescent-nav-open");
    await act(async () => {
      root.unmount();
    });
  });

  it("a click after the hover opened the nav keeps it open — it never toggles shut", async () => {
    const { container, root } = await renderShell();

    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    const navigation = container.querySelector<HTMLElement>(".crescent-nav");
    if (sliver === null || navigation === null) throw new Error("crescent did not render");
    await act(async () => {
      // React synthesizes enter/leave from over/out, so this is how a hover
      // arrives.
      sliver.dispatchEvent(new Event("pointerover", { bubbles: true }));
    });
    expect(navigation.classList).toContain("crescent-nav-open");

    await act(async () => {
      sliver.click();
    });
    expect(navigation.classList).toContain("crescent-nav-open");
    await act(async () => {
      root.unmount();
    });
  });

  it("choosing a surface closes the nav, and the focus handback does not reopen it", async () => {
    const selectSurface = vi.fn();
    useAppStore.setState({ plugins: inventory([READY]), selectSurface });
    const { container, root } = await renderShell();

    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    const navigation = container.querySelector<HTMLElement>(".crescent-nav");
    if (sliver === null || navigation === null) throw new Error("crescent did not render");
    await act(async () => sliver.focus());
    expect(navigation.classList).toContain("crescent-nav-open");

    const point = container.querySelector<HTMLButtonElement>('[aria-label="Open Polis"]');
    if (point === null) throw new Error("Polis point did not render");
    // A real click moves focus to the point first (mousedown focuses); the
    // handback to the trigger is then a focus CHANGE, which is what reopens.
    await act(async () => point.focus());
    await act(async () => point.click());

    expect(selectSurface).toHaveBeenCalledWith("polis");
    expect(navigation.classList).not.toContain("crescent-nav-open");
    expect(sliver.getAttribute("aria-expanded")).toBe("false");
    // Focus rides back to the trigger, and must not reopen what the choice
    // just closed.
    expect(document.activeElement).toBe(sliver);
    expect(navigation.classList).not.toContain("crescent-nav-open");
    await act(async () => root.unmount());
  });

  it("pointer leave with focus on a point closes it without leaving focus on hidden controls", async () => {
    const { container, root } = await renderShell();

    const main = container.querySelector("main");
    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    const navigation = container.querySelector<HTMLElement>(".crescent-nav");
    if (main === null || sliver === null || navigation === null) {
      throw new Error("crescent did not render");
    }
    await act(async () => sliver.focus());
    const point = container.querySelector<HTMLButtonElement>('[aria-label="Open Polis"]');
    if (point === null) throw new Error("Polis point did not render");
    await act(async () => point.focus());
    expect(document.activeElement).toBe(point);

    await act(async () => {
      // React synthesizes leave from out, as with enter above.
      main.dispatchEvent(new MouseEvent("pointerout", { bubbles: true }));
    });

    expect(navigation.classList).not.toContain("crescent-nav-open");
    expect(sliver.getAttribute("aria-expanded")).toBe("false");
    // Closed points are not focusable, and the focused one gave its focus up.
    expect(navigation.contains(document.activeElement)).toBe(false);
    expect(point.getAttribute("tabindex")).toBe("-1");
    await act(async () => root.unmount());
  });

  it("a closed nav leaves the accessibility tree; an open one is reachable", async () => {
    const { container, root } = await renderShell();

    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    const navigation = container.querySelector<HTMLElement>(".crescent-nav");
    if (sliver === null || navigation === null) throw new Error("crescent did not render");
    // Closed: inert removes the invisible points from the accessibility tree,
    // so browse navigation cannot find or activate them.
    expect(navigation.hasAttribute("inert")).toBe(true);

    await act(async () => sliver.focus());
    expect(navigation.classList).toContain("crescent-nav-open");
    expect(navigation.hasAttribute("inert")).toBe(false);
    const point = container.querySelector<HTMLButtonElement>('[aria-label="Open Polis"]');
    if (point === null) throw new Error("Polis point did not render");
    await act(async () => point.focus());
    expect(document.activeElement).toBe(point);

    await act(async () => {
      sliver.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(navigation.hasAttribute("inert")).toBe(true);
    expect(sliver.getAttribute("aria-expanded")).toBe("false");
    await act(async () => root.unmount());
  });

  it("a selection made while the trigger already has focus does not poison the next focus", async () => {
    const selectSurface = vi.fn();
    useAppStore.setState({ plugins: inventory([READY]), selectSurface });
    const { container, root } = await renderShell();

    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    const navigation = container.querySelector<HTMLElement>(".crescent-nav");
    if (sliver === null || navigation === null) throw new Error("crescent did not render");
    await act(async () => sliver.focus());
    expect(navigation.classList).toContain("crescent-nav-open");

    // A programmatic click moves no focus: the trigger is still
    // document.activeElement, so the handback emits no focus event and must
    // not arm the suppression flag.
    const point = container.querySelector<HTMLButtonElement>('[aria-label="Open Polis"]');
    if (point === null) throw new Error("Polis point did not render");
    await act(async () => point.click());

    expect(selectSurface).toHaveBeenCalledWith("polis");
    expect(navigation.classList).not.toContain("crescent-nav-open");

    // The next genuine focus on the trigger must still open the nav.
    await act(async () => sliver.blur());
    await act(async () => sliver.focus());
    expect(navigation.classList).toContain("crescent-nav-open");
    await act(async () => root.unmount());
  });
});
