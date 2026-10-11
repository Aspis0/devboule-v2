// The General-tab row for what Enter does while the agent runs: the choice
// persists, and a change re-renders a mounted row through the store.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { SendBehaviorSetting } from "./SendBehaviorSetting";
import { getSendBehavior, setSendBehavior } from "../../lib/sendBehavior";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

async function renderSetting(): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(<SendBehaviorSetting />);
  });
}

function radios(): NodeListOf<HTMLElement> {
  return container.querySelectorAll<HTMLElement>('[role="radio"]');
}

function optionFor(value: string): HTMLElement {
  const option = container.querySelector<HTMLElement>(
    `[role="radio"][data-choice-value="${value}"]`,
  );
  if (option === null) throw new Error(`${value} option did not render`);
  return option;
}

function isChosen(value: string): boolean {
  return optionFor(value).getAttribute("aria-checked") === "true";
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  localStorage.removeItem("devboule.sendBehavior");
  setSendBehavior("steer-and-send");
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
});

describe("SendBehaviorSetting", () => {
  it("is one shared row: the title, the two choices, no heading", async () => {
    await renderSetting();
    const row = container.querySelector("[data-settings-row]");
    expect(row?.querySelector(".settings-row-title")?.textContent).toBe(
      "Enter while the agent runs",
    );
    expect(container.querySelector("h2, h3")).toBeNull();
    expect(container.querySelector("[data-settings-row-description]")).toBeNull();
  });

  it("offers Steer, Interrupt and Queue, Steer checked by default", async () => {
    await renderSetting();
    expect(radios()).toHaveLength(3);
    expect(isChosen("steer-and-send")).toBe(true);
    expect(isChosen("interrupt-and-send")).toBe(false);
    expect(isChosen("queue")).toBe(false);
    // The alternate key is the act the composer's button names, and the copy
    // has to say so. The copy sits under the collapsed Advanced disclosure.
    expect(container.querySelector("[data-settings-advanced]")).not.toBeNull();
    expect(container.textContent).toContain(
      "When the agent is running, Enter steers the message into the running turn. Command/Ctrl+Enter queues.",
    );
    expect(container.textContent).toContain(
      "When the agent is running, Enter interrupts the turn and sends. Command/Ctrl+Enter queues.",
    );
    expect(container.textContent).toContain(
      "When the agent is running, Enter queues. Command/Ctrl+Enter interrupts the running turn and sends.",
    );
    expect(optionFor("steer-and-send").textContent).toContain("Steer");
    expect(optionFor("interrupt-and-send").textContent).toContain("Interrupt");
  });

  it("persists Steer through the store so the next surface reads it back", async () => {
    await renderSetting();
    await act(async () => optionFor("interrupt-and-send").click());
    expect(getSendBehavior()).toBe("interrupt-and-send");
    expect(isChosen("interrupt-and-send")).toBe(true);

    await act(async () => root.unmount());
    container.innerHTML = "";
    await renderSetting();
    expect(isChosen("interrupt-and-send")).toBe(true);
  });

  it("follows a change made elsewhere while the row is mounted", async () => {
    await renderSetting();
    expect(isChosen("steer-and-send")).toBe(true);
    await act(async () => setSendBehavior("interrupt-and-send"));
    expect(isChosen("interrupt-and-send")).toBe(true);
    await act(async () => setSendBehavior("queue"));
    expect(isChosen("queue")).toBe(true);
  });
});
