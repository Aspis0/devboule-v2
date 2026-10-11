// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ChoiceGroup, type ChoiceGroupProps } from "./ChoiceGroup";

const OPTIONS: ChoiceGroupProps<string>["options"] = [
  { value: "ask", label: "Ask every time" },
  { value: "tray", label: "Keep running in the tray" },
  { value: "quit", label: "Quit Devboule" },
];

function optionsOf(container: ParentNode): HTMLButtonElement[] {
  return [...container.querySelectorAll<HTMLButtonElement>('[role="radio"]')];
}

function optionNamed(container: ParentNode, label: string): HTMLButtonElement {
  const found = optionsOf(container).find((option) => option.textContent?.startsWith(label));
  if (!found) throw new Error(`no option named ${label}`);
  return found;
}

describe("ChoiceGroup", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  function render(props: Partial<ChoiceGroupProps<string>> = {}) {
    const onChange = vi.fn();
    act(() => {
      root.render(
        <ChoiceGroup
          options={OPTIONS}
          value="ask"
          onChange={onChange}
          ariaLabel="When I close the window"
          {...props}
        />,
      );
    });
    return { onChange };
  }

  it("is one radiogroup of button radios, with no native input", () => {
    render();

    const group = container.querySelector('[role="radiogroup"]');
    expect(group?.getAttribute("aria-label")).toBe("When I close the window");
    expect(optionsOf(container)).toHaveLength(3);
    expect(optionsOf(container).every((option) => option.tagName === "BUTTON")).toBe(true);
    expect(container.querySelector("input")).toBeNull();
  });

  it("names each option by its value, for tests and automation", () => {
    render();

    expect(optionsOf(container).map((option) => option.dataset.choiceValue)).toEqual([
      "ask",
      "tray",
      "quit",
    ]);
  });

  it("marks the selected option and gives the tab stop to it alone", () => {
    render({ value: "tray" });

    expect(optionNamed(container, "Keep running").getAttribute("aria-checked")).toBe("true");
    expect(optionNamed(container, "Ask every").getAttribute("aria-checked")).toBe("false");
    expect(optionNamed(container, "Keep running").tabIndex).toBe(0);
    expect(optionNamed(container, "Ask every").tabIndex).toBe(-1);
    expect(optionNamed(container, "Quit Devboule").tabIndex).toBe(-1);
  });

  it("gives the tab stop to the first enabled option when the selected one is disabled", () => {
    render({
      value: "tray",
      options: [
        { value: "ask", label: "Ask every time" },
        { value: "tray", label: "Keep running in the tray", disabled: true },
      ],
    });

    expect(optionNamed(container, "Ask every").tabIndex).toBe(0);
    expect(optionNamed(container, "Keep running").tabIndex).toBe(-1);
  });

  it("reports a click on another option, and not a click on the selected one", () => {
    const { onChange } = render();

    act(() => optionNamed(container, "Quit Devboule").click());
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith("quit");

    act(() => optionNamed(container, "Ask every").click());
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("keeps a disabled option out of reach of a click", () => {
    const { onChange } = render({
      options: [
        { value: "ask", label: "Ask every time" },
        { value: "quit", label: "Quit Devboule", disabled: true },
      ],
    });

    const quit = optionNamed(container, "Quit");
    expect(quit.getAttribute("aria-disabled")).toBe("true");
    act(() => quit.click());
    expect(onChange).not.toHaveBeenCalled();
  });

  it("shows an offline host with a dot and the word, never as a selection", () => {
    const { onChange } = render({
      options: [
        { value: "this-pc", label: "This PC" },
        { value: "tablet", label: "TABLET-V477JRIG", hint: "offline" },
      ],
      value: "this-pc",
    });

    const tablet = optionNamed(container, "TABLET");
    expect(tablet.title).toBe("TABLET-V477JRIG is offline");
    expect(tablet.querySelector(".choice-group-dot")).not.toBeNull();
    expect(tablet.querySelector(".choice-group-offline")?.textContent).toBe("offline");
    expect(tablet.getAttribute("aria-disabled")).toBe("true");
    act(() => tablet.click());
    expect(onChange).not.toHaveBeenCalled();
  });

  it("disables the whole group without hiding any option", () => {
    const { onChange } = render({ disabled: true });

    const group = container.querySelector<HTMLElement>('[role="radiogroup"]');
    expect(group?.classList.contains("is-disabled")).toBe(true);
    expect(optionsOf(container)).toHaveLength(3);
    expect(
      optionsOf(container).every((option) => option.getAttribute("aria-disabled") === "true"),
    ).toBe(true);
    act(() => optionNamed(container, "Quit Devboule").click());
    expect(onChange).not.toHaveBeenCalled();
  });

  it("lays out a vertical list only when asked", () => {
    render();
    expect(container.querySelector(".choice-group-vertical")).toBeNull();

    render({ orientation: "vertical" });
    expect(container.querySelector(".choice-group-vertical")).not.toBeNull();
  });

  it("wires the name and the description the caller names", () => {
    render({ ariaLabel: undefined, labelledBy: "row-title", describedBy: "row-hint" });

    const group = container.querySelector('[role="radiogroup"]');
    expect(group?.getAttribute("aria-labelledby")).toBe("row-title");
    expect(group?.getAttribute("aria-describedby")).toBe("row-hint");
    expect(group?.hasAttribute("aria-label")).toBe(false);
  });
});
