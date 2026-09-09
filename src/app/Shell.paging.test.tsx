// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SurfaceKey } from "../types/surface";

vi.mock("../types/surface", async () => {
  const actual = await vi.importActual<typeof import("../types/surface")>("../types/surface");
  const extras = [
    ["extra-one", "Extra One"],
    ["extra-two", "Extra Two"],
    ["extra-three", "Extra Three"],
    ["extra-four", "Extra Four"],
    ["extra-five", "Extra Five"],
    ["extra-six", "Extra Six"],
    ["extra-seven", "Extra Seven"],
  ].map(([key, label]) => ({
    key: key as SurfaceKey,
    label,
    eyebrow: "test fixture",
    description: "A named extra surface used only to exercise the paging window.",
    tone: "ochre" as const,
  }));
  return { ...actual, SURFACES: [...actual.SURFACES, ...extras] };
});

import { Shell } from "./Shell";
import { useAppStore } from "../store/appStore";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

async function renderShell(children: ReactNode = <div>Surface</div>): Promise<{
  container: HTMLDivElement;
  root: ReturnType<typeof createRoot>;
}> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<Shell activeSurface="workspace">{children}</Shell>);
  });
  return { container, root };
}

function visibleLabels(container: HTMLDivElement): string[] {
  return Array.from(container.querySelectorAll<HTMLElement>(".nav-point-label")).map(
    (label) => label.textContent ?? "",
  );
}

async function openNav(container: HTMLDivElement) {
  const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
  if (sliver === null) throw new Error("crescent did not render");
  await act(async () => sliver.focus());
}

beforeEach(() => {
  useAppStore.setState({
    installError: null,
    installing: null,
    plugins: { root: "C:/data/plugins", plugins: [], problem: null },
    refreshPlugins: vi.fn(async () => undefined),
  });
});

afterEach(() => {
  document.body.replaceChildren();
});

describe("Shell crescent paging", () => {
  it("renders paging arrows only while the nav is open", async () => {
    const { container, root } = await renderShell();

    expect(container.querySelector(".crescent-page-arrow")).toBeNull();
    await openNav(container);
    expect(container.querySelector('[aria-label="Show previous surfaces"]')).toBeNull();
    expect(container.querySelector('[aria-label="Show next surfaces"]')).not.toBeNull();

    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    if (sliver === null) throw new Error("crescent sliver missing");
    await act(async () => {
      sliver.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(container.querySelector(".crescent-page-arrow")).toBeNull();
    await act(async () => root.unmount());
  });

  it("moves the visible window by one when the next arrow is clicked", async () => {
    const { container, root } = await renderShell();
    await openNav(container);
    const next = container.querySelector<HTMLButtonElement>('[aria-label="Show next surfaces"]');
    if (next === null) throw new Error("next paging control missing");

    await act(async () => next.focus());
    await act(async () => next.click());

    expect(visibleLabels(container)[0]).toBe("Polis");
    expect(visibleLabels(container)).toContain("Extra One");
    expect(container.querySelector('[aria-label="Show previous surfaces"]')).not.toBeNull();
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Show next surfaces");
    await act(async () => root.unmount());
  });

  it("moves the visible window by one with ArrowRight and back with ArrowLeft", async () => {
    const { container, root } = await renderShell();
    await openNav(container);
    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    if (sliver === null) throw new Error("crescent sliver missing");

    await act(async () => {
      sliver.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "ArrowRight" }));
    });
    expect(visibleLabels(container)[0]).toBe("Polis");
    expect(visibleLabels(container)).toContain("Extra One");

    await act(async () => {
      sliver.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "ArrowLeft" }));
    });
    expect(visibleLabels(container)[0]).toBe("Workspace");
    expect(visibleLabels(container)).not.toContain("Extra One");
    await act(async () => root.unmount());
  });

  it("does not page from the focused sliver while the nav is closed", async () => {
    const { container, root } = await renderShell();
    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    if (sliver === null) throw new Error("crescent sliver missing");

    await openNav(container);
    await act(async () => {
      sliver.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(sliver.getAttribute("aria-expanded")).toBe("false");

    await act(async () => {
      sliver.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "ArrowRight" }));
    });

    expect(visibleLabels(container)[0]).toBe("Workspace");
    await act(async () => root.unmount());
  });

  it("reopens when focus returns to the sliver after Escape leaves it focused", async () => {
    const { container, root } = await renderShell();
    await openNav(container);
    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    if (sliver === null) throw new Error("crescent sliver missing");

    await act(async () => {
      sliver.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(sliver.getAttribute("aria-expanded")).toBe("false");

    await act(async () => {
      sliver.blur();
      sliver.focus();
    });

    expect(sliver.getAttribute("aria-expanded")).toBe("true");
    await act(async () => root.unmount());
  });

  it("keeps focus on the revealed edge when a focused point leaves the window", async () => {
    const { container, root } = await renderShell();
    await openNav(container);
    const workspace = container.querySelector<HTMLButtonElement>('[aria-label="Open Workspace"]');
    if (workspace === null) throw new Error("Workspace nav point missing");

    await act(async () => {
      workspace.focus();
      workspace.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "ArrowRight" }));
    });

    expect(document.activeElement).not.toBe(document.body);
    expect(document.activeElement?.classList.contains("nav-point")).toBe(true);
    expect(document.activeElement?.querySelector(".nav-point-label")?.textContent).toBe(
      "Extra One",
    );
    await act(async () => root.unmount());
  });

  it("keeps focus on a visible point when it survives paging", async () => {
    const { container, root } = await renderShell();
    await openNav(container);
    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    if (sliver === null) throw new Error("crescent sliver missing");

    await act(async () => {
      sliver.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "ArrowRight" }));
    });
    const marketplace = container.querySelector<HTMLButtonElement>(
      '[aria-label="Open Marketplace"]',
    );
    if (marketplace === null) throw new Error("Marketplace nav point missing");

    await act(async () => {
      marketplace.focus();
      marketplace.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "ArrowRight" }));
    });

    expect(document.activeElement?.querySelector(".nav-point-label")?.textContent).toBe(
      "Marketplace",
    );
    await act(async () => root.unmount());
  });

  it("resets the paging window when the nav closes and reopens", async () => {
    const { container, root } = await renderShell();
    await openNav(container);
    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    if (sliver === null) throw new Error("crescent sliver missing");

    await act(async () => {
      sliver.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "ArrowRight" }));
    });
    expect(visibleLabels(container)[0]).toBe("Polis");

    await act(async () => {
      sliver.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    await act(async () => {
      sliver.blur();
      sliver.focus();
    });

    expect(visibleLabels(container)[0]).toBe("Workspace");
    expect(container.querySelector('[aria-label="Show next surfaces"]')).not.toBeNull();
    await act(async () => root.unmount());
  });

  it("does not page when ArrowRight comes from an input", async () => {
    const { container, root } = await renderShell(<input aria-label="Workspace search" />);
    await openNav(container);
    const input = container.querySelector<HTMLInputElement>('input[aria-label="Workspace search"]');
    if (input === null) throw new Error("Workspace search input missing");

    await act(async () => input.focus());
    await act(async () => {
      input.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "ArrowRight" }));
    });

    expect(visibleLabels(container)[0]).toBe("Workspace");
    await act(async () => root.unmount());
  });

  it("does not page when ArrowRight comes from an input while the nav is closed", async () => {
    const { container, root } = await renderShell(<input aria-label="Workspace search" />);
    const input = container.querySelector<HTMLInputElement>('input[aria-label="Workspace search"]');
    if (input === null) throw new Error("Workspace search input missing");

    await act(async () => input.focus());
    await act(async () => {
      input.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "ArrowRight" }));
    });

    expect(visibleLabels(container)[0]).toBe("Workspace");
    expect(container.querySelector(".crescent-page-arrow")).toBeNull();
    await act(async () => root.unmount());
  });

  it("does not steal focus or prevent Escape from a closed nav input", async () => {
    const { container, root } = await renderShell(<input aria-label="Workspace search" />);
    const input = container.querySelector<HTMLInputElement>('input[aria-label="Workspace search"]');
    if (input === null) throw new Error("Workspace search input missing");

    await act(async () => input.focus());
    const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Escape" });
    await act(async () => input.dispatchEvent(event));

    expect(event.defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(input);
    await act(async () => root.unmount());
  });
});
