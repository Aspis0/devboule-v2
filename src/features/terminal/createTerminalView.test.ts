import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionSnapshot } from "../../types/ipc";

interface TerminalMockState {
  written: string[];
  writeCallbacks: Array<() => void>;
  disposeCount: number;
}

const mocks = vi.hoisted(() => {
  const state: TerminalMockState = {
    written: [],
    writeCallbacks: [],
    disposeCount: 0,
  };

  class MockTerminal {
    readonly options = { cursorBlink: false };
    readonly parser = {
      registerCsiHandler: () => ({ dispose: () => undefined }),
    };
    cols = 80;
    rows = 24;

    attachCustomKeyEventHandler(): void {}

    loadAddon(): void {}

    onData(): { dispose: () => void } {
      return { dispose: () => undefined };
    }

    open(): void {}

    reset(): void {}

    resize(cols: number, rows: number): void {
      this.cols = cols;
      this.rows = rows;
    }

    write(data: string, callback?: () => void): void {
      state.written.push(data);
      if (callback !== undefined) state.writeCallbacks.push(callback);
    }

    dispose(): void {
      state.disposeCount += 1;
    }
  }

  class MockFitAddon {
    fit(): void {}
  }

  return { MockFitAddon, MockTerminal, state };
});

vi.mock("@xterm/addon-fit", () => ({ FitAddon: mocks.MockFitAddon }));
vi.mock("@xterm/xterm", () => ({ Terminal: mocks.MockTerminal }));

import { createTerminalView } from "./createTerminalView";

const snapshot: SessionSnapshot = {
  type: "snapshot",
  asOfSeq: 4,
  cols: 12,
  rows: 3,
  data: "screen",
  cursor: { row: 1, col: 2, visible: true, shape: "underline", blinking: true },
  alternateScreen: true,
  bracketedPaste: true,
  lineWrap: false,
  title: "Terminal",
};

function completeNextWrite(): void {
  mocks.state.writeCallbacks.shift()?.();
}

describe("createTerminalView disposal", () => {
  beforeEach(() => {
    mocks.state.written.length = 0;
    mocks.state.writeCallbacks.length = 0;
    mocks.state.disposeCount = 0;
    vi.stubGlobal("getComputedStyle", () => ({ getPropertyValue: () => "" }));
    vi.stubGlobal("document", { documentElement: {} });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("completes a pending write callback once when disposed", () => {
    const view = createTerminalView({} as HTMLElement, {
      onData: () => undefined,
      onCtrlC: () => undefined,
    });
    const callback = vi.fn();

    view.write("pending", callback);
    view.dispose();
    completeNextWrite();

    expect(callback).toHaveBeenCalledTimes(1);
    expect(mocks.state.disposeCount).toBe(1);
  });

  it("completes the snapshot continuation once if disposal interrupts either write", () => {
    const view = createTerminalView({} as HTMLElement, {
      onData: () => undefined,
      onCtrlC: () => undefined,
    });
    const callback = vi.fn();

    view.applySnapshot(snapshot, callback);
    view.dispose();
    completeNextWrite();

    expect(callback).toHaveBeenCalledTimes(1);

    const secondView = createTerminalView({} as HTMLElement, {
      onData: () => undefined,
      onCtrlC: () => undefined,
    });
    const secondCallback = vi.fn();
    secondView.applySnapshot(snapshot, secondCallback);
    completeNextWrite();
    secondView.dispose();
    completeNextWrite();

    expect(secondCallback).toHaveBeenCalledTimes(1);
  });
});
