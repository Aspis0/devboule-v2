// @vitest-environment happy-dom

// The single Devboule-tools switch: one boolean per provider, always written
// with an empty deny list, plus the honest legacy notice when a stored row
// still denies tools invisibly.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ProviderToolsSwitch } from "./ProviderToolsSwitch";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("ProviderToolsSwitch", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  function switchButton(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>('[role="switch"]');
    if (!button) throw new Error("Devboule-tools switch did not render");
    return button;
  }

  async function renderSwitch(props: Partial<Parameters<typeof ProviderToolsSwitch>[0]> = {}) {
    const onToggle = props.onToggle ?? (() => {});
    const onTurnAllOn = props.onTurnAllOn ?? (() => {});
    await act(async () =>
      root.render(
        <ProviderToolsSwitch
          providerId="grok"
          enabled
          hasLegacyDenials={false}
          disabled={false}
          onToggle={onToggle}
          onTurnAllOn={onTurnAllOn}
          {...props}
        />,
      ),
    );
  }

  it("labels the switch as Devboule tools, never as the provider itself", async () => {
    await renderSwitch();
    const button = switchButton();
    expect(button.getAttribute("aria-checked")).toBe("true");
    expect(button.getAttribute("aria-label")).toMatch(/devboule tools/i);
    expect(button.getAttribute("aria-label")).toContain("grok");
    expect(container.textContent).toContain("Devboule tools");
  });

  it("reads off when the stored row disables every tool", async () => {
    await renderSwitch({ enabled: false });
    expect(switchButton().getAttribute("aria-checked")).toBe("false");
  });

  it("asks for the flipped value on toggle", async () => {
    const onToggle = vi.fn();
    await renderSwitch({ enabled: true, onToggle });
    await act(async () => switchButton().click());
    expect(onToggle).toHaveBeenCalledTimes(1);
    expect(onToggle).toHaveBeenCalledWith(false);
  });

  it("locks under the load lock and drops the click", async () => {
    const onToggle = vi.fn();
    await renderSwitch({ disabled: true, onToggle });
    expect(switchButton().disabled).toBe(true);
    await act(async () => switchButton().click());
    expect(onToggle).not.toHaveBeenCalled();
  });

  it("shows the legacy notice with a Turn-all-on action while the switch reads on", async () => {
    const onTurnAllOn = vi.fn();
    await renderSwitch({ enabled: true, hasLegacyDenials: true, onTurnAllOn });
    expect(switchButton().getAttribute("aria-checked")).toBe("true");
    expect(container.textContent).toMatch(/older setting/i);
    const action = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === "Turn all on",
    );
    if (!action) throw new Error("Turn-all-on action did not render");
    await act(async () => action.click());
    expect(onTurnAllOn).toHaveBeenCalledTimes(1);
  });

  it("shows no legacy notice when the switch already reads off", async () => {
    await renderSwitch({ enabled: false, hasLegacyDenials: true });
    expect(container.textContent).not.toMatch(/older setting/i);
    expect(container.textContent).not.toContain("Turn all on");
  });

  it("shows no legacy notice without stored denials", async () => {
    await renderSwitch({ enabled: true, hasLegacyDenials: false });
    expect(container.textContent).not.toMatch(/older setting/i);
  });
});
