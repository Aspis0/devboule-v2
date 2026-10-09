// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { getSendBehavior, setSendBehavior } from "../../../lib/sendBehavior";
import { shortcutSections } from "../../../lib/keymap";
import { ShortcutsPanel } from "./ShortcutsPanel";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("ShortcutsPanel", () => {
  let container: HTMLDivElement;
  let root: Root;

  async function renderPanel(): Promise<void> {
    root = createRoot(container);
    await act(async () => root.render(<ShortcutsPanel />));
    await act(async () => undefined);
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    setSendBehavior("queue");
  });

  afterEach(async () => {
    if (root !== undefined) await act(async () => root.unmount());
    container.remove();
    setSendBehavior("queue");
  });

  it("renders every section and row the keymap hands it", async () => {
    await renderPanel();
    const text = container.textContent ?? "";
    for (const section of shortcutSections(getSendBehavior())) {
      expect(text).toContain(section.label);
      if (section.note !== undefined) expect(text).toContain(section.note);
      for (const row of section.rows) {
        expect(text).toContain(row.keys);
        expect(text).toContain(row.title);
        if (row.detail !== undefined) expect(text).toContain(row.detail);
      }
    }
  });

  it("groups the rows under the five scopes, in order", async () => {
    await renderPanel();
    const headings = Array.from(container.querySelectorAll(".settings-section-label")).map(
      (heading) => heading.textContent,
    );
    expect(headings).toEqual(["Tabs", "Composer", "Navigation", "Panel", "Browser"]);
  });

  it("prints the strip chord and the tab-list keys", async () => {
    await renderPanel();
    const text = container.textContent ?? "";
    expect(text).toContain("Alt+Shift+]");
    expect(text).toContain("Alt+Shift+[");
    expect(text).toContain("ArrowRight / ArrowDown");
    expect(text).toContain("ArrowLeft / ArrowUp");
    expect(text).toContain("Delete / Backspace");
  });

  it("follows the send setting in the two Enter rows", async () => {
    await renderPanel();
    expect(container.textContent).toContain(
      "While the agent is working, Enter queues the message when queueing is available; otherwise it sends.",
    );
    expect(container.textContent).toContain("Interrupt and send");

    await act(async () => setSendBehavior("interrupt-and-send"));

    expect(container.textContent).toContain(
      "While the agent is working, Enter interrupts the turn and sends the message.",
    );
    expect(container.textContent).toContain("Queue the message");
    expect(container.textContent).toContain(
      "While the agent is working, this queues the message when queueing is available; otherwise the draft stays in the composer.",
    );
    expect(container.textContent).toContain("otherwise the draft stays in the composer");
    expect(container.textContent).not.toContain("otherwise it sends");
  });

  it("renders the one blocked-states note under the Composer group", async () => {
    await renderPanel();
    expect(container.textContent).toContain(
      "Nothing is sent or queued while the composer is disabled or an image send is in progress.",
    );
  });
});
