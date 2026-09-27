// The Notifications page: the two switches the attention toasts obey.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { NotificationsSection } from "./NotificationsSection";
import { getShowMessagePreviews, getShowNotifications } from "../../lib/notificationPrefs";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root | null = null;

function renderSection(): void {
  root = createRoot(container);
  act(() => {
    root!.render(<NotificationsSection />);
  });
}

function switchFor(name: string): HTMLButtonElement {
  const found = container.querySelector<HTMLButtonElement>(`[role="switch"][aria-label="${name}"]`);
  if (found === null) throw new Error(`${name} switch did not render`);
  return found;
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  localStorage.clear();
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  container.remove();
  localStorage.clear();
});

describe("NotificationsSection", () => {
  it("offers Show notifications and Show message previews, both on by default", () => {
    renderSection();
    const master = switchFor("Show notifications");
    const previews = switchFor("Show message previews");
    expect(master.getAttribute("aria-checked")).toBe("true");
    expect(previews.getAttribute("aria-checked")).toBe("true");
    expect(previews.disabled).toBe(false);
  });

  it("persists the master switch through the store", () => {
    renderSection();
    act(() => {
      switchFor("Show notifications").click();
    });
    expect(getShowNotifications()).toBe(false);
    expect(switchFor("Show notifications").getAttribute("aria-checked")).toBe("false");
  });

  it("locks the previews switch while the master is off", () => {
    renderSection();
    act(() => {
      switchFor("Show notifications").click();
    });
    // No previews without toasts: the switch locks instead of silently
    // keeping a value that does nothing.
    expect(switchFor("Show message previews").disabled).toBe(true);
    act(() => {
      switchFor("Show notifications").click();
    });
    expect(switchFor("Show message previews").disabled).toBe(false);
  });

  it("persists the previews switch through the store", () => {
    renderSection();
    act(() => {
      switchFor("Show message previews").click();
    });
    expect(getShowMessagePreviews()).toBe(false);
    expect(switchFor("Show message previews").getAttribute("aria-checked")).toBe("false");
  });

  it("says the previews reach the lock screen", () => {
    renderSection();
    expect(container.textContent).toContain("lock screen");
  });
});
