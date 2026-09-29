// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentTaskItem } from "../../lib/agentSession";
import { AgentTaskPill } from "./AgentTaskPill";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

/** Synthetic checklist: one of each status, the middle one with an activeForm. */
const ITEMS: AgentTaskItem[] = [
  { id: "t-1", text: "Read the journal", status: "completed" },
  {
    id: "t-2",
    text: "Replay the frames",
    status: "in_progress",
    activeForm: "Replaying the frames",
  },
  { id: "t-3", text: "Write the report", status: "pending" },
];

/** What the wire is allowed to carry is not what a newer daemon will send. */
const UNKNOWN_ITEM = {
  id: "u-1",
  text: "Await the vendor patch",
  status: "blocked",
} as unknown as AgentTaskItem;

async function renderPill(items: readonly AgentTaskItem[]): Promise<void> {
  root = createRoot(container);
  await act(async () => root.render(<AgentTaskPill items={items} />));
}

async function rerenderPill(items: readonly AgentTaskItem[]): Promise<void> {
  await act(async () => root.render(<AgentTaskPill items={items} />));
}

function toggle(): HTMLButtonElement {
  const button = container.querySelector<HTMLButtonElement>(
    '[data-testid="agent-task-pill-toggle"]',
  );
  if (button === null) throw new Error("no toggle rendered");
  return button;
}

async function clickToggle(): Promise<void> {
  await act(async () => toggle().click());
}

function headText(): string {
  return toggle().textContent ?? "";
}

function list(): HTMLUListElement {
  const element = container.querySelector<HTMLUListElement>('[data-testid="agent-task-pill-list"]');
  if (element === null) throw new Error("no list rendered");
  return element;
}

function statusWords(): string[] {
  return [...list().querySelectorAll(".agent-task-row-status")].map((word) => word.textContent);
}

describe("AgentTaskPill", () => {
  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.restoreAllMocks();
  });

  describe("the collapsed head copy", () => {
    it("names the running step as the current step, with no next: prefix", async () => {
      await renderPill(ITEMS);

      const head = headText();
      expect(head).toContain("1 of 3");
      expect(head).toContain("Replaying the frames");
      expect(head).not.toContain("next:");
      expect(list().hasAttribute("hidden")).toBe(true);
    });

    it("falls back to the item's text when the running item has no activeForm", async () => {
      await renderPill([{ text: "Kick off the run", status: "in_progress" }]);

      const head = headText();
      expect(head).toContain("Kick off the run");
      expect(head).not.toContain("next:");
    });

    it("counts the still-running steps after the first as more running", async () => {
      await renderPill([
        { id: "t-1", text: "Read the journal", status: "completed" },
        { id: "t-2", text: "Replay the frames", status: "in_progress" },
        { id: "t-3", text: "Check the fixture", status: "in_progress" },
      ]);

      expect(headText()).toContain("+1 more running");
    });

    it("says next: only when nothing is running and a pending item is left", async () => {
      await renderPill([
        { id: "t-1", text: "Read the journal", status: "completed" },
        { id: "t-3", text: "Write the report", status: "pending" },
      ]);

      const head = headText();
      expect(head).toContain("next: Write the report");
      expect(head).toContain("1 of 2");
    });

    it("says All done once every item is completed", async () => {
      await renderPill([
        { id: "t-1", text: "Read the journal", status: "completed" },
        { id: "t-2", text: "Write the report", status: "completed" },
      ]);

      const head = headText();
      expect(head).toContain("2 of 2");
      expect(head).toContain("All done");
    });

    it("titles the collapsed line with the item's full text", async () => {
      await renderPill(ITEMS);

      const current = container.querySelector(".agent-task-pill-current");
      expect(current?.getAttribute("title")).toBe("Replay the frames");
    });
  });

  it("renders nothing when the session has no list", async () => {
    await renderPill([]);

    expect(container.firstChild).toBeNull();
  });

  it("expands to one row per item, each with its status word for the screen reader", async () => {
    await renderPill(ITEMS);

    await clickToggle();

    expect(list().hasAttribute("hidden")).toBe(false);
    const rows = [...list().querySelectorAll(".agent-task-row")];
    expect(rows.map((row) => row.getAttribute("data-status"))).toEqual([
      "completed",
      "in_progress",
      "pending",
    ]);
    expect(statusWords()).toEqual(["Done", "In progress", "Pending"]);
    expect(rows[1].textContent).toContain("Replay the frames");
  });

  it("gives an unknown status its own mark and its own status word", async () => {
    await renderPill([UNKNOWN_ITEM]);

    await clickToggle();

    const row = list().querySelector(".agent-task-row");
    expect(row?.getAttribute("data-status")).toBe("unknown");
    const mark = row?.querySelector(".agent-task-mark");
    expect(mark?.className).toContain("agent-task-mark--unknown");
    expect(mark?.className).not.toContain("--pending");
    expect(row?.querySelector(".agent-task-row-status")?.textContent).toBe("Unknown");
  });

  it("toggles aria-expanded on one button, and the list holds no controls", async () => {
    await renderPill(ITEMS);

    expect(toggle().tagName).toBe("BUTTON");
    expect(toggle().getAttribute("type")).toBe("button");
    expect(toggle().getAttribute("aria-expanded")).toBe("false");

    await clickToggle();
    expect(toggle().getAttribute("aria-expanded")).toBe("true");

    await clickToggle();
    expect(toggle().getAttribute("aria-expanded")).toBe("false");

    await clickToggle();
    // The marks are status, not checkboxes: the whole surface is one button —
    // the toggle — and the list contains none of it.
    expect(container.querySelectorAll("button")).toHaveLength(1);
    expect(list().querySelectorAll("button, input, a")).toHaveLength(0);
  });

  it("closes on Escape with the focus on the toggle", async () => {
    await renderPill(ITEMS);
    await clickToggle();
    const head = toggle();
    head.focus();

    await act(async () => {
      head.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    expect(list().hasAttribute("hidden")).toBe(true);
    expect(document.activeElement).toBe(head);
  });

  it("arrives collapsed after its list was cleared", async () => {
    await renderPill(ITEMS);
    await clickToggle();
    expect(toggle().getAttribute("aria-expanded")).toBe("true");

    await rerenderPill([]);
    await rerenderPill([{ id: "n-1", text: "Fresh plan", status: "pending" }]);

    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    expect(list().hasAttribute("hidden")).toBe(true);
  });

  it("keys rows uniquely when the wire repeats an id", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => undefined);

    await renderPill([
      { id: "dup", text: "First", status: "pending" },
      { id: "dup", text: "Second", status: "pending" },
    ]);

    const keyWarnings = errors.mock.calls.filter((args) =>
      args.some((arg) => String(arg).includes("same key")),
    );
    expect(keyWarnings).toEqual([]);
  });
});
