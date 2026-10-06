// What Escape does in the composer: it stops a running turn, which is what the
// working line says it does, and it never takes the key from the command menu.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { composerDrivers, composerProps, type ComposerDrivers } from "./composerTestKit";
import { WorkspaceComposer } from "./WorkspaceComposer";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;
let drive: ComposerDrivers;
const onStop = vi.fn();

async function renderComposer(streaming: boolean): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(
      <WorkspaceComposer
        {...composerProps(
          { onSend: vi.fn(async () => true), onQueue: vi.fn() },
          { streaming, turnActive: streaming, onStop },
        )}
      />,
    );
  });
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  drive = composerDrivers(container);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  onStop.mockClear();
});

describe("Escape in the composer", () => {
  it("stops a turn that is running", async () => {
    await renderComposer(true);

    const event = await drive.press("Escape");

    expect(onStop).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(true);
  });

  it("does nothing when no turn is running", async () => {
    await renderComposer(false);

    const event = await drive.press("Escape");

    expect(onStop).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });

  it("closes the command menu first, and leaves the turn alone", async () => {
    await renderComposer(true);
    await drive.type("/b");
    expect(drive.menu()).not.toBeNull();

    await drive.press("Escape");

    expect(drive.menu()).toBeNull();
    expect(onStop).not.toHaveBeenCalled();
  });
});
