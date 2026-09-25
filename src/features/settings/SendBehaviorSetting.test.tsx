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

function radios(): NodeListOf<HTMLInputElement> {
  return container.querySelectorAll<HTMLInputElement>('input[type="radio"]');
}

function radioFor(value: string): HTMLInputElement {
  const input = container.querySelector<HTMLInputElement>(`input[value="${value}"]`);
  if (input === null) throw new Error(`${value} option did not render`);
  return input;
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  localStorage.removeItem("devboule.sendBehavior");
  setSendBehavior("queue");
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
});

describe("SendBehaviorSetting", () => {
  it("offers Queue and Steer in the General tab's row form, Queue checked by default", async () => {
    await renderSetting();
    // The row every other General setting uses — not the page-heading class a
    // tab-level title wears (review F17).
    const row = container.querySelector(".settings-card.settings-value-row");
    expect(row).not.toBeNull();
    expect(row?.textContent).toContain("Default send");
    expect(container.querySelector("h2")).toBeNull();
    expect(radios()).toHaveLength(2);
    expect(radioFor("queue").checked).toBe(true);
    expect(radioFor("interrupt-and-send").checked).toBe(false);
    // The alternate key is the act the composer's button names, and the copy
    // has to say so: under the queue default it interrupts the running turn.
    expect(container.textContent).toContain(
      "When the agent is running, Enter queues. Command/Ctrl+Enter interrupts the running turn and sends.",
    );
    expect(container.textContent).toContain(
      "When the agent is running, Enter interrupts. Command/Ctrl+Enter queues.",
    );
    expect(radioFor("interrupt-and-send").closest("label")?.textContent).toContain("Steer");
  });

  it("persists Steer through the store so the next surface reads it back", async () => {
    await renderSetting();
    await act(async () => radioFor("interrupt-and-send").click());
    expect(getSendBehavior()).toBe("interrupt-and-send");
    expect(radioFor("interrupt-and-send").checked).toBe(true);

    await act(async () => root.unmount());
    container.innerHTML = "";
    await renderSetting();
    expect(radioFor("interrupt-and-send").checked).toBe(true);
  });

  it("follows a change made elsewhere while the row is mounted", async () => {
    await renderSetting();
    expect(radioFor("queue").checked).toBe(true);
    await act(async () => setSendBehavior("interrupt-and-send"));
    expect(radioFor("interrupt-and-send").checked).toBe(true);
    await act(async () => setSendBehavior("queue"));
    expect(radioFor("queue").checked).toBe(true);
  });
});
