// The composer's Enter contract: what Enter does with the menu closed (send,
// queue, steer per Q2b), what the Ctrl/Cmd chord does with it open and closed,
// and the two keys that never send — Shift+Enter's newline and an open IME
// composition. The menu's own keys are `WorkspaceComposer.commandMenu.test.tsx`;
// the surface-level proof stays in `AgentChatSurface.queue.test.tsx`.
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
let mocks: ComposerMocks;
let drive: ComposerDrivers;

async function renderComposer(
  overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {},
): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(<WorkspaceComposer {...composerProps(mocks, overrides)} />);
  });
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  onSend = vi.fn<(text: string) => void>();
  onQueue = vi.fn<(text: string) => void>();
  mocks = { onSend, onQueue };
  drive = composerDrivers(container);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("Enter with the menu closed", () => {
  it("sends, as it always has", async () => {
    await renderComposer();
    await drive.type("hello");
    expect(drive.menu()).toBeNull();

    await drive.press("Enter");

    expect(onSend).toHaveBeenCalledWith("hello");
    expect(drive.textarea().value).toBe("");
  });

  it("queues on Enter while the turn runs when Enter is set to queue", async () => {
    await renderComposer({ turnActive: true, enterQueues: true });
    await drive.type("later");

    await drive.press("Enter");

    expect(onQueue).toHaveBeenCalledWith("later");
    expect(onSend).not.toHaveBeenCalled();
  });

  it("queues on the alternate chord when Enter is set to steer", async () => {
    await renderComposer({ turnActive: true, enterQueues: false });
    await drive.type("later");

    await drive.press("Enter", { ctrlKey: true });

    expect(onQueue).toHaveBeenCalledWith("later");
    expect(onSend).not.toHaveBeenCalled();
  });

  it("sends on the alternate chord when Enter is set to queue", async () => {
    await renderComposer({ turnActive: true, enterQueues: true });
    await drive.type("later");

    await drive.press("Enter", { ctrlKey: true });

    expect(onSend).toHaveBeenCalledWith("later");
    expect(onQueue).not.toHaveBeenCalled();
  });

  it("queues a slash-leading message with more than a command in it", async () => {
    // Whitespace ends the token, so the menu never opens for a message that
    // merely starts with a slash: this is the path a queued follow-up takes.
    await renderComposer({ turnActive: true, enterQueues: true });
    await drive.type("/remember to also bump the changelog");
    expect(drive.menu()).toBeNull();

    await drive.press("Enter");

    expect(onQueue).toHaveBeenCalledWith("/remember to also bump the changelog");
    expect(drive.textarea().value).toBe("");
  });
});

describe("Enter and the chord with the menu open", () => {
  it("hands Ctrl and Cmd+Enter to the queue instead of inserting a row", async () => {
    await renderComposer({ turnActive: true, enterQueues: false });
    await drive.type("/go");
    expect(drive.menu()).not.toBeNull();

    await drive.press("Enter", { ctrlKey: true });

    expect(onQueue).toHaveBeenCalledWith("/go");
    expect(onSend).not.toHaveBeenCalled();
    expect(drive.textarea().value).toBe("");

    await drive.type("/go");
    await drive.press("Enter", { metaKey: true });

    expect(onQueue).toHaveBeenCalledTimes(2);
    expect(onQueue).toHaveBeenLastCalledWith("/go");
    expect(drive.textarea().value).toBe("");
  });

  it("hands Ctrl+Enter to the interrupting send under the queue setting", async () => {
    await renderComposer({ turnActive: true, enterQueues: true });
    await drive.type("/go");

    await drive.press("Enter", { ctrlKey: true });

    expect(onSend).toHaveBeenCalledWith("/go");
    expect(onQueue).not.toHaveBeenCalled();
  });

  it("completes on a plain Enter even when Enter is set to queue", async () => {
    await renderComposer({ turnActive: true, enterQueues: true });
    await drive.type("/go");

    await drive.press("Enter");

    expect(drive.textarea().value).toBe("/goal ");
    expect(onQueue).not.toHaveBeenCalled();
    expect(onSend).not.toHaveBeenCalled();
  });
});

describe("the keys that never send", () => {
  it("keeps Shift+Enter as the newline the Send button promises, menu open or not", async () => {
    await renderComposer();
    await drive.type("/go");
    expect(drive.menu()).not.toBeNull();

    const shiftEnter = await drive.press("Enter", { shiftKey: true });

    expect(shiftEnter.defaultPrevented).toBe(false);
    expect(drive.menu()).not.toBeNull();
    expect(drive.textarea().value).toBe("/go");
    expect(onSend).not.toHaveBeenCalled();
  });

  it("leaves Enter to an open composition: no insert, no send", async () => {
    await renderComposer();
    await drive.type("/go");

    const composing = await drive.press("Enter", { isComposing: true });
    expect(composing.defaultPrevented).toBe(false);
    expect(drive.textarea().value).toBe("/go");
    expect(drive.menu()).not.toBeNull();
    expect(onSend).not.toHaveBeenCalled();

    // Older engines report the composition commit as keyCode 229 alone.
    const legacy = await drive.press("Enter", { keyCode: 229 });
    expect(legacy.defaultPrevented).toBe(false);
    expect(drive.textarea().value).toBe("/go");
    expect(onSend).not.toHaveBeenCalled();
  });
});
