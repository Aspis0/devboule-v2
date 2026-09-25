// Human-path proofs for the queue rows: steer, edit, delete, drag and the
// Alt+Arrow keyboard, the error a refused send leaves on the row, and the
// focus that follows a removed row to its neighbour — or reports that the
// track emptied.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { QueuedMessage } from "./messageQueue";
import { QUEUE_ROW_MIME, QueueTrack } from "./QueueTrack";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

function item(id: string, text: string, error?: string): QueuedMessage {
  return {
    id,
    text,
    attachments: [],
    // The row's wire identity: what the queue composes, what the rows never
    // read, and what a test building a row by hand still has to name.
    idempotencyKey: `s.test.1.${id}.00000000-0000-4000-8000-000000000000`,
    ...(error === undefined ? {} : { error }),
  };
}

function makeHandlers() {
  return {
    onSteer: vi.fn<(id: string) => void>(),
    onEdit: vi.fn<(id: string) => void>(),
    onDelete: vi.fn<(id: string) => void>(),
    onMove: vi.fn<(id: string, index: number) => void>(),
    onEmptied: vi.fn<() => void>(),
  };
}

type Handlers = ReturnType<typeof makeHandlers>;

async function renderTrack(
  items: QueuedMessage[],
  handlers: Handlers = makeHandlers(),
): Promise<Handlers> {
  root = createRoot(container);
  await act(async () => {
    root.render(
      <QueueTrack
        items={items}
        onSteer={handlers.onSteer}
        onEdit={handlers.onEdit}
        onDelete={handlers.onDelete}
        onMove={handlers.onMove}
        onEmptied={handlers.onEmptied}
      />,
    );
  });
  return handlers;
}

async function rerenderTrack(items: QueuedMessage[], handlers: Handlers): Promise<void> {
  await act(async () => {
    root.render(
      <QueueTrack
        items={items}
        onSteer={handlers.onSteer}
        onEdit={handlers.onEdit}
        onDelete={handlers.onDelete}
        onMove={handlers.onMove}
        onEmptied={handlers.onEmptied}
      />,
    );
  });
}

function rows(): NodeListOf<HTMLDivElement> {
  return container.querySelectorAll('[data-testid="queue-row"]');
}

function rowButton(row: HTMLDivElement, testId: string): HTMLButtonElement {
  const button = row.querySelector<HTMLButtonElement>(`[data-testid="${testId}"]`);
  if (button === null) throw new Error(`${testId} button did not render`);
  return button;
}

/** A hand-built dataTransfer: happy-dom has no drag session of its own. */
function dragSession() {
  const carried = new Map<string, string>();
  return {
    get types() {
      return [...carried.keys()];
    },
    effectAllowed: "all",
    setData: (kind: string, value: string) => carried.set(kind, value),
    getData: (kind: string) => carried.get(kind) ?? "",
  };
}

function fireDrag(element: Element, type: string, transfer: ReturnType<typeof dragSession>) {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(event, "dataTransfer", { value: transfer });
  element.dispatchEvent(event);
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
});

describe("QueueTrack", () => {
  it("renders nothing when the queue is empty", async () => {
    await renderTrack([]);
    expect(container.querySelector('[data-testid="queue-track"]')).toBeNull();
    expect(container.firstElementChild).toBeNull();
  });

  it("shows each message on the row with steer, edit and delete buttons", async () => {
    await renderTrack([item("q1", "run the tests"), item("q2", "then commit")]);
    const track = container.querySelector('[data-testid="queue-track"]');
    expect(track).not.toBeNull();
    expect(track?.getAttribute("aria-label")).toBe("Queued messages");
    expect(rows()).toHaveLength(2);
    expect(rows()[0].textContent).toContain("run the tests");
    expect(rowButton(rows()[0], "queue-steer").getAttribute("aria-label")).toBe(
      "Send queued message now — interrupts the running turn",
    );
    expect(rowButton(rows()[0], "queue-edit").getAttribute("aria-label")).toBe(
      "Edit queued message",
    );
    expect(rowButton(rows()[0], "queue-delete").getAttribute("aria-label")).toBe(
      "Delete queued message",
    );
  });

  it("renders the reason a refused send left on the item", async () => {
    await renderTrack([item("q1", "stuck", "the daemon refused this message")]);
    const note = rows()[0].querySelector('[role="alert"]');
    expect(note?.textContent).toBe("the daemon refused this message");
  });

  it("tells the keyboard how a row is reordered", async () => {
    await renderTrack([item("q1", "one")]);
    const describedBy = rows()[0].getAttribute("aria-describedby");
    expect(describedBy).toBeTruthy();
    const hint = document.getElementById(describedBy ?? "");
    expect(hint?.textContent).toContain("Alt");
    expect(hint?.textContent).toContain("ArrowUp");
    expect(hint?.textContent).toContain("ArrowDown");
  });

  it("steer sends the row now, edit and delete name their row", async () => {
    const handlers = await renderTrack([item("q1", "one"), item("q2", "two")]);
    await act(async () => rowButton(rows()[1], "queue-steer").click());
    expect(handlers.onSteer).toHaveBeenCalledWith("q2");
    await act(async () => rowButton(rows()[0], "queue-edit").click());
    expect(handlers.onEdit).toHaveBeenCalledWith("q1");
    await act(async () => rowButton(rows()[0], "queue-delete").click());
    expect(handlers.onDelete).toHaveBeenCalledWith("q1");
  });

  it("reorders by drag: dropping a row on another lands it in front of that row", async () => {
    const handlers = await renderTrack([item("q1", "one"), item("q2", "two"), item("q3", "three")]);
    const transfer = dragSession();
    await act(async () => fireDrag(rows()[0], "dragstart", transfer));
    await act(async () => fireDrag(rows()[2], "drop", transfer));
    // Review F15: `move` counts the destination in the list the dragged row has
    // already left, so a downward drop asked for index 2 landed the row one
    // past the one it was dropped on. The gesture now asks for the row's own
    // slot, and the row dropped on ends up just below it — as it does when the
    // drag goes upward.
    expect(handlers.onMove).toHaveBeenCalledWith("q1", 1);
    expect(transfer.types).toContain(QUEUE_ROW_MIME);
    expect(transfer.types).not.toContain("text/plain");
  });

  it("lands an upward drop in front of the row it was dropped on, too", async () => {
    const handlers = await renderTrack([item("q1", "one"), item("q2", "two"), item("q3", "three")]);
    const transfer = dragSession();
    await act(async () => fireDrag(rows()[2], "dragstart", transfer));
    await act(async () => fireDrag(rows()[0], "drop", transfer));
    expect(handlers.onMove).toHaveBeenCalledWith("q3", 0);
  });

  it("says the reorder instruction once, outside the list, and shows none of it", async () => {
    await renderTrack([item("q1", "one"), item("q2", "two")]);
    const hint = container.querySelector<HTMLSpanElement>(".workspace-queue-hint");
    if (hint === null) throw new Error("the reorder instruction did not render");
    // A node inside a list that is not a list item is an invalid child, and a
    // whole tutorial rendered at 10 px above the rows is not a hint (F14).
    expect(hint.closest('[role="list"]')).toBeNull();
    const list = container.querySelector('[role="list"]');
    expect(list?.firstElementChild?.getAttribute("data-testid")).toBe("queue-row");
    expect(list?.querySelector(".workspace-queue-hint")).toBeNull();
    expect(rows()).toHaveLength(2);
    // Every row still names it, and it is named once for the whole track.
    expect(rows()[0].getAttribute("aria-describedby")).toBe(hint.id);
  });

  it("ignores a drop that carries only plain text, as a dragged selection does", async () => {
    const handlers = await renderTrack([item("q1", "one"), item("q2", "two")]);
    const transfer = dragSession();
    transfer.setData("text/plain", "some sentence dragged from the transcript");
    await act(async () => fireDrag(rows()[1], "dragover", transfer));
    await act(async () => fireDrag(rows()[1], "drop", transfer));
    expect(handlers.onMove).not.toHaveBeenCalled();
  });

  it("ignores a drop that carries nothing at all", async () => {
    const handlers = await renderTrack([item("q1", "one")]);
    const transfer = dragSession();
    await act(async () => fireDrag(rows()[0], "drop", transfer));
    expect(handlers.onMove).not.toHaveBeenCalled();
  });

  it("reorders from the keyboard with Alt+ArrowUp and Alt+ArrowDown on a focused row", async () => {
    const handlers = await renderTrack([item("q1", "one"), item("q2", "two")]);
    const row = rows()[1];
    expect(row.getAttribute("tabindex")).toBe("0");
    await act(async () =>
      row.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowUp", altKey: true, bubbles: true }),
      ),
    );
    expect(handlers.onMove).toHaveBeenCalledWith("q2", 0);
    await act(async () =>
      row.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", altKey: true, bubbles: true }),
      ),
    );
    expect(handlers.onMove).toHaveBeenCalledWith("q2", 2);
  });

  it("leaves plain arrow keys without Alt to the browser", async () => {
    const handlers = await renderTrack([item("q1", "one")]);
    await act(async () =>
      rows()[0].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true })),
    );
    await act(async () =>
      rows()[0].dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", altKey: false, bubbles: true }),
      ),
    );
    expect(handlers.onMove).not.toHaveBeenCalled();
  });

  it("moves focus to the next row when a middle row is removed", async () => {
    const handlers = await renderTrack([item("q1", "one"), item("q2", "two"), item("q3", "three")]);
    rows()[1].focus();
    await act(async () => rowButton(rows()[1], "queue-delete").click());
    await rerenderTrack([item("q1", "one"), item("q3", "three")], handlers);
    expect(document.activeElement).toBe(rows()[1]);
    expect(rows()[1].getAttribute("data-queue-id")).toBe("q3");
  });

  it("moves focus to the next row when the first row is removed", async () => {
    const handlers = await renderTrack([item("q1", "one"), item("q2", "two"), item("q3", "three")]);
    rows()[0].focus();
    await act(async () => rowButton(rows()[0], "queue-delete").click());
    await rerenderTrack([item("q2", "two"), item("q3", "three")], handlers);
    expect(document.activeElement).toBe(rows()[0]);
    expect(rows()[0].getAttribute("data-queue-id")).toBe("q2");
  });

  it("moves focus to the previous row when the last row is removed", async () => {
    const handlers = await renderTrack([item("q1", "one"), item("q2", "two")]);
    rows()[1].focus();
    await act(async () => rowButton(rows()[1], "queue-delete").click());
    await rerenderTrack([item("q1", "one")], handlers);
    expect(document.activeElement).toBe(rows()[0]);
    expect(rows()[0].getAttribute("data-queue-id")).toBe("q1");
  });

  it("reports to the composer when the row taken out was the only one", async () => {
    const handlers = await renderTrack([item("q1", "one")]);
    rows()[0].focus();
    await act(async () => rowButton(rows()[0], "queue-delete").click());
    await rerenderTrack([], handlers);
    expect(handlers.onEmptied).toHaveBeenCalledTimes(1);
  });

  it("leaves focus alone when the row removed was not holding it", async () => {
    const handlers = await renderTrack([item("q1", "one"), item("q2", "two")]);
    await act(async () => rowButton(rows()[0], "queue-delete").click());
    await rerenderTrack([item("q2", "two")], handlers);
    expect(document.activeElement).toBe(document.body);
  });
});
