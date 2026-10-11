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
    expect(container.querySelector("[data-settings-row-description]")).not.toBeNull();
  });

  it("offers Steer, Interrupt and Queue, Steer checked by default", async () => {
    await renderSetting();
    expect(radios()).toHaveLength(3);
    expect(isChosen("steer-and-send")).toBe(true);
    expect(isChosen("interrupt-and-send")).toBe(false);
    expect(isChosen("queue")).toBe(false);
    expect(optionFor("steer-and-send").textContent).toContain("Steer");
    expect(optionFor("interrupt-and-send").textContent).toContain("Interrupt");
  });

  it("states the selected option's line only, with the command key of this platform", async () => {
    await renderSetting();
    const line = container.querySelector("[data-settings-row-description]")?.textContent ?? "";
    expect(line).toMatch(/^Enter adds your message to the running turn · (Ctrl\+|⌘)Enter queues$/);
    expect(container.querySelector("[data-settings-advanced]")).toBeNull();
    expect(container.querySelector('[role="radiogroup"]')?.getAttribute("aria-describedby")).toBe(
      container.querySelector("[data-settings-row-description]")?.id,
    );

    await act(async () => optionFor("queue").click());
    expect(container.querySelector("[data-settings-row-description]")?.textContent).toMatch(
      /^Enter waits for the turn to end · (Ctrl\+|⌘)Enter stops and sends$/,
    );
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
