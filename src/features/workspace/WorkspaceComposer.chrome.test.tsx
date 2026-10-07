// The composer chrome's look contract: the send control is a button named
// "Send" that sends, the running turn replaces it with a still-named Stop that
// interrupts, and the queue action's icon follows its label — the queue mark
// while it queues, the send arrow while the interrupt-and-send default steers. The keys' contract is `WorkspaceComposer.sendKeys.test.tsx`; the trigger
// labels' proof is the provider·model chip tests in `AgentChatSurface.test.tsx`.
// @vitest-environment happy-dom
import { act, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import {
  composerDrivers,
  composerProps,
  type ComposerDrivers,
  type ComposerMocks,
} from "./composerTestKit";
import { assembleCssProof, removeCssProof } from "./cssProof";
import type { PromptAttachment } from "../../types/ipc";
import { WorkspaceComposer } from "./WorkspaceComposer";

const rootDir = resolve(import.meta.dirname, "../../..");
const workspaceCss = assembleCssProof([
  readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/features/workspace/Workspace.css"), "utf8"),
]);

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;
let onSend: Mock<(text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>>;
let onQueue: Mock<(text: string, attachments: readonly PromptAttachment[]) => void>;
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
  onSend = vi.fn<(text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>>();
  onQueue = vi.fn<(text: string, attachments: readonly PromptAttachment[]) => void>();
  onStop = vi.fn<() => void>();
  mocks = { onSend, onQueue, onStop };
  drive = composerDrivers(container);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  removeCssProof();
  vi.clearAllMocks();
});

describe("the composer's box", () => {
  it("floors itself at one line plus the control row", async () => {
    // The floor names a height: 10 of top padding, one 14px line at 1.45, the
    // bar's 6px gap and its 28px height, and 10 of bottom padding.
    const floor = workspaceCss.rulesFor(".workspace-composer");
    expect(floor).toContain("min-height: calc(");
    expect(floor).toContain("(14px * 1.45)");
    expect(floor).toContain("28px + 10px");
    expect(floor).not.toContain("border: 1px");
    expect(floor).not.toContain("border-radius");
  });

  it("draws one full-width top rule on the wrap, and takes accent on focus", () => {
    const wrap = workspaceCss.rulesFor(".workspace-composer-wrap");
    expect(wrap).toContain("border-top: 1px solid");
    expect(workspaceCss.rulesFor(".workspace-composer-wrap:focus-within")).toContain(
      "border-top-color:",
    );
  });

  it("renders the textarea as a block, so no strut sits under its line", async () => {
    workspaceCss.inject([".workspace-composer textarea"]);
    await renderComposer();
    const textarea = container.querySelector("textarea");
    if (textarea === null) throw new Error("composer textarea did not render");
    // An inline-block textarea sits on a line box, and the strut's descender
    // left 5.3px of dead height under the only line of text.
    expect(getComputedStyle(textarea).display).toBe("block");
  });
});

describe("the send control", () => {
  it("is a button named Send that sends the typed text", async () => {
    await renderComposer();
    await drive.type("hello");

    const send = container.querySelector<HTMLButtonElement>(".workspace-send-action");
    if (send === null) throw new Error("send button did not render");
    expect(send.getAttribute("aria-label")).toBe("Send");

    await act(async () => send.click());

    expect(onSend).toHaveBeenCalledWith("hello", []);
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
  it("draws a clock while the action queues", async () => {
    await renderComposer({ turnActive: true, enterQueues: true });

    const queue = container.querySelector<HTMLButtonElement>(
      '[data-testid="composer-queue-action"]',
    );
    if (queue === null) throw new Error("queue action did not render");
    expect(queue.getAttribute("aria-label")).toBe("Queue message");
    const circle = queue.querySelector("circle");
    // Face centred on the 24-unit box at Send's scale.
    expect([
      circle?.getAttribute("cx"),
      circle?.getAttribute("cy"),
      circle?.getAttribute("r"),
    ]).toEqual(["12", "12", "8"]);
    const paths = [...queue.querySelectorAll("path")].map((path) => path.getAttribute("d"));
    // Hands from the centre, up and right: a 12-to-3 shape.
    expect(paths).toEqual(["M12 12V7", "M12 12H17"]);
  });

  it("wears the send arrow while the action interrupts", async () => {
    // The interrupt-and-send default (enterQueues false) with no pending
    // permission: the button says "Send and interrupt" and steers, which
    // sends — so it draws what it does, not the queue mark. (A pending
    // permission unmounts this button instead: queueAllowed goes false.)
    await renderComposer({ turnActive: true });

    const queue = container.querySelector<HTMLButtonElement>(
      '[data-testid="composer-queue-action"]',
    );
    if (queue === null) throw new Error("queue action did not render");
    expect(queue.getAttribute("aria-label")).toBe("Send and interrupt");
    const paths = [...queue.querySelectorAll("path")].map((path) => path.getAttribute("d"));
    expect(paths).toEqual(["M12 19V5", "m5 12 7-7 7 7"]);
    // No stream runs here, so the plain send button stands beside it: two
    // identical send arrows, both sending. Named so the pair reads as the
    // interrupt state's normal chrome, not a doubled control.
    const buttons = container.querySelectorAll(
      '[data-testid="composer-queue-action"], .workspace-send-action',
    );
    expect(buttons).toHaveLength(2);
    for (const button of buttons) {
      const drawn = [...button.querySelectorAll("path")].map((path) => path.getAttribute("d"));
      expect(drawn).toEqual(["M12 19V5", "m5 12 7-7 7 7"]);
    }
  });

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

    expect(onQueue).toHaveBeenCalledWith("later", []);
    expect(onSend).not.toHaveBeenCalled();
  });
});
