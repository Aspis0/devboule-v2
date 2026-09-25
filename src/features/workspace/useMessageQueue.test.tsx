// The chat surface's side of the queue, driven through a probe component:
// snapshots land, every action reaches the queue, and every refusal lands
// where the user reads it — the error line, or the composer's text back.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SEND_FAILED } from "./inMemoryMessageQueue";
import { createQueueHarness, flushQueueTurns, type QueueHarness } from "./queueHarness";
import { useMessageQueue, type MessageQueueUiHandlers } from "./useMessageQueue";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

function renderProbe(harness: QueueHarness | null) {
  const handlers: MessageQueueUiHandlers = {
    onEditRestored: vi.fn(),
    onSteerRefused: vi.fn(),
    onQueueRefused: vi.fn(),
  };
  function Probe() {
    const ui = useMessageQueue(harness?.queue ?? null, handlers);
    return (
      <div
        data-testid="probe"
        data-items={JSON.stringify(
          ui.items.map((item) => ({ id: item.id, text: item.text, error: item.error })),
        )}
        data-error={ui.error ?? ""}
      >
        <button
          type="button"
          data-testid="queue-it"
          onClick={() => ui.queueMessage("from the probe")}
        />
        <button type="button" data-testid="queue-blank" onClick={() => ui.queueMessage("   ")} />
        <button type="button" data-testid="edit-it" onClick={() => ui.editRow("queued-1")} />
        <button type="button" data-testid="delete-it" onClick={() => ui.deleteRow("queued-1")} />
        <button type="button" data-testid="steer-row" onClick={() => ui.steerRow("queued-1")} />
        <button type="button" data-testid="move-it" onClick={() => ui.moveRow("queued-1", 1)} />
        <button
          type="button"
          data-testid="steer-composer"
          onClick={() => ui.steerComposer("steered words")}
        />
        <button type="button" data-testid="steer-blank" onClick={() => ui.steerComposer("   ")} />
      </div>
    );
  }
  root = createRoot(container);
  act(() => {
    root.render(<Probe />);
  });
  const snapshot = () => {
    const probe = container.querySelector<HTMLDivElement>('[data-testid="probe"]');
    if (probe === null) throw new Error("probe did not render");
    return {
      items: JSON.parse(probe.dataset.items ?? "[]") as {
        id: string;
        text: string;
        error?: string;
      }[],
      error: probe.dataset.error === "" ? null : probe.dataset.error,
    };
  };
  return { handlers, snapshot };
}

async function click(testId: string): Promise<void> {
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`[data-testid="${testId}"]`)?.click();
    await flushQueueTurns();
  });
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
});

describe("useMessageQueue", () => {
  it("carries the queue's snapshots into render state", async () => {
    const harness = createQueueHarness();
    const probe = renderProbe(harness);
    expect(probe.snapshot().items).toEqual([]);
    await act(async () => {
      harness.queue.add("hello", []);
    });
    expect(probe.snapshot().items.map((item) => item.text)).toEqual(["hello"]);
  });

  it("renders nothing and does nothing without a queue", async () => {
    const probe = renderProbe(null);
    await click("queue-it");
    expect(probe.snapshot().items).toEqual([]);
    expect(probe.snapshot().error).toBeNull();
  });

  it("takes the composer's text into the queue, and hands back what the queue refuses", async () => {
    const harness = createQueueHarness();
    const probe = renderProbe(harness);

    await click("queue-it");
    expect(probe.snapshot().items.map((item) => item.text)).toEqual(["from the probe"]);
    expect(probe.handlers.onQueueRefused).not.toHaveBeenCalled();
    expect(probe.snapshot().error).toBeNull();

    // The refusal the queue can raise: text with nothing in it.
    await click("queue-blank");
    expect(probe.snapshot().error).toBe("There is nothing to queue.");
    expect(probe.handlers.onQueueRefused).toHaveBeenCalledWith("   ");

    await click("queue-it");
    expect(probe.snapshot().error).toBeNull();
  });

  it("hands the row's text back once the queue has taken it", async () => {
    const harness = createQueueHarness();
    harness.queue.add("a row", []);
    harness.status = "idle";
    const probe = renderProbe(harness);

    await click("edit-it");
    expect(probe.handlers.onEditRestored).toHaveBeenCalledWith("a row");
    expect(probe.snapshot().items).toEqual([]);
  });

  it("says so when the row the user clicked is already gone", async () => {
    const harness = createQueueHarness();
    harness.queue.add("a row", []);
    const probe = renderProbe(harness);
    act(() => {
      harness.queue.take("queued-1"); // drained or deleted a moment ago
    });

    await click("edit-it");
    expect(probe.handlers.onEditRestored).not.toHaveBeenCalled();
    expect(probe.snapshot().error).toBe("That queued message is gone.");
  });

  it("deletes and reorders through the queue, and a delete of nothing says nothing", async () => {
    const harness = createQueueHarness();
    harness.queue.add("a row", []);
    harness.queue.add("another", []);
    const probe = renderProbe(harness);

    await click("move-it");
    expect(probe.snapshot().items.map((item) => item.text)).toEqual(["another", "a row"]);

    await click("delete-it");
    expect(probe.snapshot().items.map((item) => item.text)).toEqual(["another"]);

    await click("delete-it"); // already gone: the goal is reached
    expect(probe.snapshot().items.map((item) => item.text)).toEqual(["another"]);
    expect(probe.snapshot().error).toBeNull();
  });

  it("sends a row steer through the queue", async () => {
    const harness = createQueueHarness();
    harness.queue.add("a row", []);
    harness.status = "idle";
    const probe = renderProbe(harness);

    await click("steer-row");
    expect(harness.actions).toEqual(["send:a row"]);
    expect(probe.snapshot().items).toEqual([]);
    expect(probe.handlers.onSteerRefused).not.toHaveBeenCalled();
    expect(probe.snapshot().error).toBeNull();
  });

  // Review F16: one alert per failure. The row is where a queued send's
  // refusal is said; the composer line stays for the steer the queue never
  // took at all.
  it("a refused row steer keeps the row and stamps it, without a second alert", async () => {
    const harness = createQueueHarness();
    harness.queue.add("stuck row", []);
    harness.status = "idle";
    const probe = renderProbe(harness);

    harness.failingSends = true;
    await click("steer-row");
    expect(probe.snapshot().items[0].error).toBe(SEND_FAILED);
    expect(probe.snapshot().error).toBeNull();
    expect(probe.handlers.onSteerRefused).not.toHaveBeenCalled();
  });

  it("a composer steer whose send the session refused stays on its row", async () => {
    const harness = createQueueHarness();
    harness.status = "idle";
    const probe = renderProbe(harness);

    await click("steer-composer");
    expect(harness.actions).toEqual(["send:steered words"]);
    expect(probe.handlers.onSteerRefused).not.toHaveBeenCalled();
    harness.status = "idle"; // the turn that steer opened is over

    // A refusal is not a lost message: the text is a row now, with its reason,
    // and the ladder retries it. The composer keeps what it was told to send
    // only when the queue refused to take it at all.
    harness.failingSends = true;
    await click("steer-composer");
    expect(probe.handlers.onSteerRefused).not.toHaveBeenCalled();
    expect(harness.current().map((item) => item.text)).toEqual(["steered words"]);
    expect(harness.current()[0].error).toBe(SEND_FAILED);
  });

  it("text the queue refuses to take goes back to the composer", async () => {
    const harness = createQueueHarness();
    const probe = renderProbe(harness);
    await click("steer-blank");
    expect(probe.handlers.onSteerRefused).toHaveBeenCalledWith("   ");
    expect(probe.snapshot().error).toBe("There is nothing to send.");
  });
});
