// The composer chrome's look contract: the send control is a button named
// "Send" that sends, the running turn replaces it with a still-named Stop that
// interrupts, and the queue action keeps its label through the icon change.
// The keys' contract is `WorkspaceComposer.sendKeys.test.tsx`; the trigger
// labels' proof is the provider·model chip tests in `AgentChatSurface.test.tsx`.
// @vitest-environment happy-dom
import { act, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import {
  composerDrivers,
  composerProps,
  type ComposerDrivers,
  type ComposerMocks,
} from "./composerTestKit";
import { WorkspaceComposer } from "./WorkspaceComposer";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;
let onSend: Mock<(text: string) => void>;
let onQueue: Mock<(text: string) => void>;
let onStop: Mock<() => void>;
let mocks: ComposerMocks & { onStop: () => void };
let drive: ComposerDrivers;

async function renderComposer(
  overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {},
): Promise<void> {
  // Unmount whatever a previous render left, so one case can drive the
  // composer through several states (send, stop, queue) in sequence.
  await act(async () => {
    root?.unmount();
  });
  root = createRoot(container);
  await act(async () => {
    root.render(<WorkspaceComposer {...composerProps(mocks, overrides)} onStop={mocks.onStop} />);
  });
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  onSend = vi.fn<(text: string) => void>();
  onQueue = vi.fn<(text: string) => void>();
  onStop = vi.fn<() => void>();
  mocks = { onSend, onQueue, onStop };
  drive = composerDrivers(container);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("the send control", () => {
  it("is a button named Send that sends the typed text", async () => {
    await renderComposer();
    await drive.type("hello");

    const send = container.querySelector<HTMLButtonElement>(".workspace-send-action");
    if (send === null) throw new Error("send button did not render");
    expect(send.getAttribute("aria-label")).toBe("Send");

    await act(async () => send.click());

    expect(onSend).toHaveBeenCalledWith("hello");
  });

  it("is absent while the turn runs, where Stop takes its place", async () => {
    await renderComposer({ streaming: true });

    expect(container.querySelector(".workspace-send-action")).toBeNull();
    const stop = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Stop the current turn"]',
    );
    if (stop === null) throw new Error("stop button did not render");

    await act(async () => stop.click());

    expect(onStop).toHaveBeenCalledTimes(1);
    expect(onSend).not.toHaveBeenCalled();
  });

  it("stays disabled with an empty composer", async () => {
    await renderComposer();

    const send = container.querySelector<HTMLButtonElement>(".workspace-send-action");
    expect(send?.disabled).toBe(true);
  });
});

describe("the action icons", () => {
  const ICON_STATES: {
    overrides: Partial<ComponentProps<typeof WorkspaceComposer>>;
    button: string;
  }[] = [
    { overrides: {}, button: ".workspace-send-action" },
    { overrides: { streaming: true }, button: ".workspace-stop-action" },
    { overrides: { turnActive: true }, button: '[data-testid="composer-queue-action"]' },
  ];

  it("draws each action icon with the repo convention: stroke currentColor, no fill", async () => {
    for (const state of ICON_STATES) {
      await renderComposer(state.overrides);
      const icon = container.querySelector(`${state.button} svg`);
      expect(icon, `${state.button} did not render an icon`).not.toBeNull();
      // Without these attributes SVG defaults fill to black and stroke to
      // none, and the colour tokens never reach the icon in either theme.
      expect(icon?.getAttribute("fill")).toBe("none");
      expect(icon?.getAttribute("stroke")).toBe("currentColor");
    }
  });
});

describe("the queue action", () => {
  it("keeps its label as the accessible name and queues on click", async () => {
    await renderComposer({ turnActive: true, enterQueues: true });
    await drive.type("later");

    const queue = container.querySelector<HTMLButtonElement>(
      '[data-testid="composer-queue-action"]',
    );
    if (queue === null) throw new Error("queue action did not render");
    expect(queue.getAttribute("aria-label")).toBe("Queue message");
    expect(queue.disabled).toBe(false);

    await act(async () => queue.click());

    expect(onQueue).toHaveBeenCalledWith("later");
    expect(onSend).not.toHaveBeenCalled();
  });
});
