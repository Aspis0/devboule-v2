// A send that crosses a link must not eat the prompt: with `confirmSend`
// the composer waits for the text-only answer and hands the text back on
// failure, instead of the fire-and-forget the local surface keeps.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import {
  composerDrivers,
  composerProps,
  type ComposerDrivers,
  type ComposerMocks,
} from "./composerTestKit";
import type { PromptAttachment } from "../../types/ipc";
import { WorkspaceComposer } from "./WorkspaceComposer";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;
let onSend: Mock<(text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>>;
let mocks: ComposerMocks;
let drive: ComposerDrivers;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  onSend = vi.fn<(text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>>();
  mocks = { onSend, onQueue: vi.fn() };
  drive = composerDrivers(container);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("confirmSend", () => {
  it("keeps fire-and-forget without the flag", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<WorkspaceComposer {...composerProps(mocks)} />);
    });
    onSend.mockResolvedValue(false);
    await drive.type("hello");
    await drive.press("Enter");
    // Not awaited: the input clears on the press, whatever the answer.
    expect(drive.textarea().value).toBe("");
    expect(onSend).toHaveBeenCalledWith("hello", []);
  });

  it("hands the text back when a confirmed send fails", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<WorkspaceComposer {...composerProps(mocks, { confirmSend: true })} />);
    });
    let settle!: (sent: boolean) => void;
    onSend.mockReturnValue(new Promise<boolean>((resolve) => (settle = resolve)));
    await drive.type("hello remote");
    await drive.press("Enter");
    // Cleared while the send is in flight...
    expect(drive.textarea().value).toBe("");
    await act(async () => {
      settle(false);
    });
    // ...and handed back on failure.
    expect(drive.textarea().value).toBe("hello remote");
  });

  it("clears a confirmed send that succeeds", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<WorkspaceComposer {...composerProps(mocks, { confirmSend: true })} />);
    });
    onSend.mockResolvedValue(true);
    await drive.type("hello remote");
    await drive.press("Enter");
    expect(drive.textarea().value).toBe("");
  });
});
