// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppSelect } from "./AppSelect";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const OPTIONS = [
  { value: "claude", label: "claude" },
  { value: "codex", label: "codex" },
  { value: "grok", label: "grok", disabled: true },
];

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

async function render(onChange = vi.fn(), disabled = false): Promise<ReturnType<typeof vi.fn>> {
  await act(async () =>
    root.render(
      <AppSelect
        label="Provider"
        value="claude"
        options={OPTIONS}
        onChange={onChange}
        disabled={disabled}
      />,
    ),
  );
  return onChange;
}

function trigger(): HTMLButtonElement {
  const found = container.querySelector<HTMLButtonElement>('button[aria-haspopup="listbox"]');
  if (found === null) throw new Error("select trigger missing");
  return found;
}

function option(value: string): HTMLButtonElement {
  const found = container.querySelector<HTMLButtonElement>(
    `[role="option"][data-value="${value}"]`,
  );
  if (found === null) throw new Error(`option ${value} missing`);
  return found;
}

describe("AppSelect", () => {
  it("names the field and shows the current option's label on its trigger", async () => {
    await render();
    expect(trigger().getAttribute("aria-label")).toBe("Provider");
    expect(trigger().textContent).toContain("claude");
    expect(trigger().getAttribute("aria-expanded")).toBe("false");
    expect(container.querySelector('[role="listbox"]')).toBeNull();
  });

  it("opens a listbox of its options and picks one, reporting the value and closing", async () => {
    const onChange = await render();
    await act(async () => trigger().click());

    expect(container.querySelector('[role="listbox"]')?.getAttribute("aria-label")).toBe(
      "Provider",
    );
    await act(async () => option("codex").click());

    expect(onChange).toHaveBeenCalledWith("codex");
    expect(container.querySelector('[role="listbox"]')).toBeNull();
  });

  it("marks the current option selected and never picks a disabled one", async () => {
    const onChange = await render();
    await act(async () => trigger().click());

    expect(option("claude").getAttribute("aria-selected")).toBe("true");
    expect(option("grok").disabled).toBe(true);
    await act(async () => option("grok").click());
    expect(onChange).not.toHaveBeenCalled();
  });

  it("closes on Escape without picking", async () => {
    const onChange = await render();
    await act(async () => trigger().click());
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(container.querySelector('[role="listbox"]')).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("does not open while disabled", async () => {
    await render(vi.fn(), true);
    await act(async () => trigger().click());

    expect(trigger().disabled).toBe(true);
    expect(container.querySelector('[role="listbox"]')).toBeNull();
  });
});
