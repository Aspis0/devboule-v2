// Escape in the agent pane stops the turn that is running — once, never on a
// held key, and never when a card is waiting or something else owns the key.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { channelHarness } from "./sessionChannelHarness";

vi.mock("../../lib/tauri", async () => (await import("./sessionChannelHarness")).tauriMock);

import { sessionInterrupt } from "../../lib/tauri";
import { AgentChatSurface } from "./AgentChatSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  channelHarness.emit = null;
  channelHarness.active = null;
  channelHarness.activeSubscriptionId = null;
  channelHarness.nextSubscriptionId = 41;
  channelHarness.deferNextAttach = false;
  channelHarness.releaseNextAttach = null;
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  document.querySelectorAll("[data-test-overlay]").forEach((node) => node.remove());
  vi.clearAllMocks();
});

async function mount(
  props: { hasPendingPermission?: boolean; activity?: "working" | "idle" } = {},
): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId="esc-agent"
        title="Agent"
        observedState={{ type: "live", generation: 1 }}
        {...props}
      />,
    );
  });
  await act(async () => undefined);
}

async function typeDraft(text: string): Promise<void> {
  const textarea = composer() as HTMLTextAreaElement;
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  await act(async () => {
    setValue?.call(textarea, text);
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

/** Sends a prompt through the composer, which is what starts a turn this view runs. */
async function startTurn(): Promise<void> {
  const textarea = container.querySelector<HTMLTextAreaElement>(
    'textarea[aria-label="Message the agent"]',
  );
  const send = container.querySelector<HTMLButtonElement>(".workspace-send-action");
  if (textarea === null || send === null) throw new Error("the composer did not render");
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  setValue?.call(textarea, "Go");
  textarea.dispatchEvent(new Event("input", { bubbles: true }));
  await act(async () => send.click());
  expect(container.querySelector(".workspace-working-line")).not.toBeNull();
}

async function press(target: Element, init: KeyboardEventInit = {}): Promise<KeyboardEvent> {
  const event = new KeyboardEvent("keydown", {
    key: "Escape",
    bubbles: true,
    cancelable: true,
    ...init,
  });
  await act(async () => {
    target.dispatchEvent(event);
  });
  return event;
}

const composer = () => {
  const textarea = container.querySelector('textarea[aria-label="Message the agent"]');
  if (textarea === null) throw new Error("no composer");
  return textarea;
};
const transcript = () => {
  const region = container.querySelector(".workspace-conversation");
  if (region === null) throw new Error("no transcript");
  return region;
};

describe("Escape in the agent pane", () => {
  it("stops a running turn from the composer and from the transcript alike", async () => {
    await mount();
    await startTurn();

    const event = await press(composer());
    expect(vi.mocked(sessionInterrupt)).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(true);

    // The agent answers the interrupt, the turn ends, and a new turn is stoppable again.
    await act(async () => {
      channelHarness.active?.({ type: "agent_finished", stopReason: "cancelled", modelId: "m" });
    });
    await startTurn();
    await press(transcript());
    expect(vi.mocked(sessionInterrupt)).toHaveBeenCalledTimes(2);
  });

  it("sends one interrupt for a turn, however often the key is pressed", async () => {
    await mount();
    await startTurn();

    await press(composer());
    await press(composer());
    await press(transcript());

    expect(vi.mocked(sessionInterrupt)).toHaveBeenCalledTimes(1);
  });

  it("ignores a held key", async () => {
    await mount();
    await startTurn();

    const event = await press(composer(), { repeat: true });

    expect(vi.mocked(sessionInterrupt)).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });

  it("does nothing when no turn is running", async () => {
    await mount();

    await press(composer());

    expect(vi.mocked(sessionInterrupt)).not.toHaveBeenCalled();
  });

  it("does nothing while a permission or question card is waiting", async () => {
    await mount({ hasPendingPermission: true });
    await startTurn();

    await press(composer());

    expect(vi.mocked(sessionInterrupt)).not.toHaveBeenCalled();
  });

  it("leaves the key to a menu, list or dialog that is open", async () => {
    await mount();
    await startTurn();
    for (const role of ["menu", "listbox", "dialog"]) {
      const overlay = document.createElement("div");
      overlay.setAttribute("role", role);
      overlay.setAttribute("data-test-overlay", "");
      document.body.append(overlay);

      await press(composer());

      expect(vi.mocked(sessionInterrupt), role).not.toHaveBeenCalled();
      overlay.remove();
    }
  });

  it("leaves the key to a person who is typing, and takes it again once the draft is empty or blank", async () => {
    await mount();
    await startTurn();

    await typeDraft("a half-written follow-up");
    await press(composer());
    await press(transcript());
    expect(vi.mocked(sessionInterrupt)).not.toHaveBeenCalled();

    await typeDraft("   ");
    await press(composer());
    expect(vi.mocked(sessionInterrupt)).toHaveBeenCalledTimes(1);
  });

  it("leaves the key to something that already handled it, and to an IME composition", async () => {
    await mount();
    await startTurn();
    const handled = (event: Event) => event.preventDefault();
    composer().addEventListener("keydown", handled, { once: true });
    await press(composer());
    expect(vi.mocked(sessionInterrupt)).not.toHaveBeenCalled();

    await press(composer(), { isComposing: true });
    expect(vi.mocked(sessionInterrupt)).not.toHaveBeenCalled();
  });
});

describe("a session that was already running when this view attached", () => {
  async function replayRunningTurn(): Promise<void> {
    await act(async () => {
      channelHarness.active?.({ type: "agent_message", messageId: "r-1", text: "Mid-turn output" });
    });
  }

  it("shows the working line without a clock, and Escape stops it", async () => {
    await mount({ activity: "working" });
    await replayRunningTurn();

    const line = container.querySelector(".workspace-working-line");
    expect(line).not.toBeNull();
    // No send was made here, so no start is known and none is claimed.
    expect(line?.querySelector(".workspace-working-clock")).toBeNull();

    await press(composer());
    expect(vi.mocked(sessionInterrupt)).toHaveBeenCalledTimes(1);
  });

  it("shows no line for a session the roster does not call working", async () => {
    await mount({ activity: "idle" });
    await replayRunningTurn();

    expect(container.querySelector(".workspace-working-line")).toBeNull();
    await press(composer());
    expect(vi.mocked(sessionInterrupt)).not.toHaveBeenCalled();
  });
});
