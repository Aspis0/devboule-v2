// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ChoiceGroup, type ChoiceGroupProps } from "./ChoiceGroup";

const OPTIONS: ChoiceGroupProps<string>["options"] = [
  { value: "steer", label: "Steer" },
  { value: "interrupt", label: "Interrupt" },
  { value: "queue", label: "Queue" },
];

describe("ChoiceGroup keyboard", () => {
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
          value="steer"
          onChange={onChange}
          ariaLabel="Enter while the agent runs"
          {...props}
        />,
      );
    });
    return { onChange };
  }

  function buttons(): HTMLButtonElement[] {
    return [...container.querySelectorAll<HTMLButtonElement>('[role="radio"]')];
  }

  function press(target: HTMLElement, key: string): KeyboardEvent {
    const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
    act(() => {
      target.dispatchEvent(event);
    });
    return event;
  }

  it("moves focus and selects the next option on ArrowRight", () => {
    const { onChange } = render();

    press(buttons()[0], "ArrowRight");

    expect(document.activeElement).toBe(buttons()[1]);
    expect(onChange).toHaveBeenCalledExactlyOnceWith("interrupt");
  });

  it("treats ArrowDown like ArrowRight", () => {
    const { onChange } = render({ value: "interrupt" });

    press(buttons()[1], "ArrowDown");

    expect(onChange).toHaveBeenCalledExactlyOnceWith("queue");
  });

  it("treats ArrowUp like ArrowLeft", () => {
    const { onChange } = render({ value: "queue" });

    press(buttons()[2], "ArrowUp");

    expect(onChange).toHaveBeenCalledExactlyOnceWith("interrupt");
  });

  it("skips a disabled option", () => {
    const { onChange } = render({
      options: [
        { value: "steer", label: "Steer" },
        { value: "interrupt", label: "Interrupt", disabled: true },
        { value: "queue", label: "Queue" },
      ],
    });

    press(buttons()[0], "ArrowRight");

    expect(document.activeElement).toBe(buttons()[2]);
    expect(onChange).toHaveBeenCalledExactlyOnceWith("queue");
  });

  it("wraps from the last option to the first, as native radios do", () => {
    const { onChange } = render({ value: "queue" });

    press(buttons()[2], "ArrowRight");

    expect(document.activeElement).toBe(buttons()[0]);
    expect(onChange).toHaveBeenCalledExactlyOnceWith("steer");
  });

  it("jumps to the first and last enabled option on Home and End", () => {
    const { onChange } = render({ value: "interrupt" });

    press(buttons()[1], "End");
    expect(onChange).toHaveBeenLastCalledWith("queue");

    press(buttons()[2], "Home");
    expect(onChange).toHaveBeenLastCalledWith("steer");
  });

  it("leaves other keys alone", () => {
    const { onChange } = render();

    const event = press(buttons()[0], "a");

    expect(event.defaultPrevented).toBe(false);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("does not report a change when the only enabled option is already selected", () => {
    const { onChange } = render({
      options: [
        { value: "steer", label: "Steer" },
        { value: "interrupt", label: "Interrupt", disabled: true },
      ],
    });

    press(buttons()[0], "ArrowRight");

    expect(onChange).not.toHaveBeenCalled();
  });
});
