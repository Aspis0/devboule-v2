// @vitest-environment happy-dom

// The provider menu's cap is measured while open, not assumed: the shell
// takes menuPlacement's room above its trigger as an inline max-height, a
// window resize re-measures it, and a below-side result — the side this
// upward menu never opens on — leaves the CSS fallback cap alone. Happy-dom
// has no layout, so the trigger's rectangle and the menu's content height
// are stubbed the way PickerChip's placement test stubs them.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ProviderInfo } from "../../types/ipc";
import { DesignAgentPicker } from "./DesignAgentPicker";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** The 22-row shell's content height, measured in Chrome. */
const NATURAL = 819;

function provider(id: string): ProviderInfo {
  return {
    id,
    executable: id,
    acpAvailable: true,
    authentication: "unknown",
    protocol: "acp",
    origin: "user-binary",
  };
}

function makeRect(left: number, top: number, right: number, bottom: number): DOMRect {
  return {
    left,
    top,
    right,
    bottom,
    width: right - left,
    height: bottom - top,
    x: left,
    y: top,
    toJSON: () => undefined,
  } as DOMRect;
}

let root: ReturnType<typeof createRoot> | null = null;
let container: HTMLDivElement | null = null;

beforeEach(() => {
  // menuPlacement reads the menu's height with the cap lifted; happy-dom has
  // no layout, so the natural content height arrives from here.
  vi.spyOn(Element.prototype, "scrollHeight", "get").mockReturnValue(NATURAL);
});

afterEach(async () => {
  vi.restoreAllMocks();
  if (root !== null) {
    const mounted = root;
    root = null;
    await act(async () => mounted.unmount());
  }
  container?.remove();
  container = null;
});

async function renderPicker(cardRect?: DOMRect): Promise<HTMLButtonElement> {
  const host = document.createElement("div");
  if (cardRect !== undefined) {
    // The design tree clips the menu through this box, not the window.
    host.className = "surface-card";
    host.getBoundingClientRect = () => cardRect;
  }
  document.body.appendChild(host);
  container = host;
  root = createRoot(host);
  await act(async () => {
    root!.render(
      <DesignAgentPicker
        providers={[provider("claude"), provider("codex"), provider("grok")]}
        providersLoading={false}
        selectedProviderId={null}
        unavailableProviderId={null}
        busy={false}
        agentSession={null}
        agentState={null}
        onProviderSelect={() => undefined}
        onModelSelect={() => undefined}
        onEffortSelect={() => undefined}
      />,
    );
  });
  const trigger = host.querySelector<HTMLButtonElement>('button[aria-label^="Choose provider:"]');
  if (trigger === null) throw new Error("provider trigger did not render");
  return trigger;
}

async function openPicker(trigger: HTMLButtonElement): Promise<HTMLElement> {
  await act(async () => trigger.click());
  const menu = document.getElementById("design-provider-picker");
  if (menu === null) throw new Error("provider picker did not open");
  return menu;
}

/** Stands the trigger where the composer puts it: `top` px from the top. */
function anchorTriggerAt(trigger: HTMLButtonElement, top: number): void {
  trigger.getBoundingClientRect = () => makeRect(50, top, 250, top + 28);
}

describe("the provider menu's measured cap", () => {
  it("caps the open menu by the room above its trigger", async () => {
    const trigger = await renderPicker();
    anchorTriggerAt(trigger, window.innerHeight - 100);
    const menu = await openPicker(trigger);
    // The helper's room: trigger top − its 8px viewport margin − its 6px
    // anchor gap. This menu's own CSS gap is 8px, so the written cap sits
    // 6px inside the room — conservative on purpose.
    expect(menu.style.maxHeight).toBe(`${window.innerHeight - 114}px`);
  });

  it("caps to the clipping card's room, not the window's", async () => {
    const trigger = await renderPicker(makeRect(0, 13, window.innerWidth, window.innerHeight - 13));
    anchorTriggerAt(trigger, window.innerHeight - 100);
    const menu = await openPicker(trigger);
    // The card starts at y = 13 (the crescent band): its room is 13px
    // smaller than the window's, so the cap must be too — otherwise the
    // shell's top edge lands 7px above the clip and gets cut there.
    expect(menu.style.maxHeight).toBe(`${window.innerHeight - 127}px`);
  });

  it("re-measures when the window resizes while the menu is open", async () => {
    const trigger = await renderPicker();
    anchorTriggerAt(trigger, window.innerHeight - 100);
    const menu = await openPicker(trigger);
    expect(menu.style.maxHeight).toBe(`${window.innerHeight - 114}px`);
    anchorTriggerAt(trigger, window.innerHeight - 300);
    await act(async () => {
      window.dispatchEvent(new Event("resize"));
    });
    expect(menu.style.maxHeight).toBe(`${window.innerHeight - 314}px`);
  });

  it("keeps the CSS fallback cap when the helper would open below", async () => {
    const trigger = await renderPicker();
    // Near the top: the content fits neither side and the below side is the
    // larger one, so menuPlacement reports below — a side this menu never
    // opens on, and its number would be the wrong room.
    anchorTriggerAt(trigger, 100);
    const menu = await openPicker(trigger);
    expect(menu.style.maxHeight).toBe("");
  });

  it("clears a stale cap when a later measure declines the above side", async () => {
    const trigger = await renderPicker();
    anchorTriggerAt(trigger, window.innerHeight - 100);
    const menu = await openPicker(trigger);
    expect(menu.style.maxHeight).toBe(`${window.innerHeight - 114}px`);
    anchorTriggerAt(trigger, 100);
    await act(async () => {
      window.dispatchEvent(new Event("resize"));
    });
    // The decline must not leave the old number standing: the shell goes
    // back to the CSS fallback.
    expect(menu.style.maxHeight).toBe("");
  });

  it("removes the resize listener when the menu closes", async () => {
    const added = vi.spyOn(window, "addEventListener");
    const removed = vi.spyOn(window, "removeEventListener");
    const trigger = await renderPicker();
    anchorTriggerAt(trigger, window.innerHeight - 100);
    await openPicker(trigger);
    await act(async () => trigger.click());
    const resizeAdds = added.mock.calls.filter((call) => call[0] === "resize");
    const resizeRemoves = removed.mock.calls.filter((call) => call[0] === "resize");
    expect(resizeAdds).toHaveLength(1);
    expect(resizeRemoves).toHaveLength(1);
    expect(resizeRemoves[0]?.[1]).toBe(resizeAdds[0]?.[1]);
    expect(document.getElementById("design-provider-picker")).toBeNull();
  });
});
