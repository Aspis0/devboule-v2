import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionSnapshot } from "../../types/ipc";

interface FitProposal {
  cols: number;
  rows: number;
}

interface TerminalMockState {
  written: string[];
  writeCallbacks: Array<() => void>;
  disposeCount: number;
  fitCount: number;
  terminals: Array<{ cols: number; rows: number }>;
  fitAddons: Array<{ proposal: FitProposal | null }>;
}

const mocks = vi.hoisted(() => {
  const state: TerminalMockState = {
    written: [],
    writeCallbacks: [],
    disposeCount: 0,
    fitCount: 0,
    terminals: [],
    fitAddons: [],
  };

  class MockTerminal {
    readonly options = { cursorBlink: false };
    readonly parser = {
      registerCsiHandler: () => ({ dispose: () => undefined }),
    };
    cols = 80;
    rows = 24;

    constructor() {
      state.terminals.push(this);
    }

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
    proposal: FitProposal | null = null;

    constructor() {
      state.fitAddons.push(this);
    }

    fit(): void {
      state.fitCount += 1;
    }

    proposeDimensions(): FitProposal | null {
      return this.proposal;
    }
  }

  return { MockFitAddon, MockTerminal, state };
});

vi.mock("@xterm/addon-fit", () => ({ FitAddon: mocks.MockFitAddon }));
vi.mock("@xterm/xterm", () => ({ Terminal: mocks.MockTerminal }));

import { createTerminalView, terminalTheme } from "./createTerminalView";

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

describe("terminalTheme", () => {
  it("paints the terminal's one surface: the ground token for the background, under the cursor, and as black", () => {
    // The resolver echoes each variable's own name, so any entry that reads
    // the wrong token names itself in the failure.
    const theme = terminalTheme((variable) => variable.slice(2));
    expect(theme.background).toBe("terminal-ground");
    expect(theme.cursorAccent).toBe("terminal-ground");
    expect(theme.black).toBe("terminal-ground");
    expect(theme.foreground).toBe("code-text");
    expect(theme.cursor).toBe("accent");
    expect(theme.selectionBackground).toBe("fill-selected");
    expect(theme.brightWhite).toBe("lb-text");
  });
});

describe("the viewport paint (static CSS contract)", () => {
  it("paints .xterm-viewport from the terminal ground, not xterm.css's default black", () => {
    // This xterm never inlines the theme background on the viewport, so the
    // leftover pixel under the fitted screen showed xterm.css's #000 as a
    // line at the box's bottom edge. The rule is what is painted, and it must
    // outrank xterm.css's `.xterm .xterm-viewport`, which loads later.
    const css = readFileSync(resolve(import.meta.dirname, "../workspace/Workspace.css"), "utf8");
    const block =
      /\.workspace-terminal-host \.xterm \.xterm-viewport\s*\{([^}]*)\}/.exec(css)?.[1] ?? "";
    expect(block, "a scoped .xterm-viewport rule is missing").not.toBe("");
    expect(block).toContain("background: var(--terminal-ground)");
  });
});

describe("fitting", () => {
  beforeEach(() => {
    mocks.state.fitCount = 0;
    mocks.state.terminals.length = 0;
    mocks.state.fitAddons.length = 0;
    vi.stubGlobal("getComputedStyle", () => ({ getPropertyValue: () => "0px" }));
    vi.stubGlobal("document", { documentElement: {}, fonts: { ready: Promise.resolve() } });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("routes the font-ready refit through onFontFit, so the session resizes the PTY", async () => {
    let resolveFonts: (() => void) | null = null;
    vi.stubGlobal("getComputedStyle", () => ({ getPropertyValue: () => "0px" }));
    vi.stubGlobal("document", {
      documentElement: {},
      fonts: { ready: new Promise<void>((resolve) => (resolveFonts = resolve)) },
    });
    const onFontFit = vi.fn();
    const host = { querySelector: () => null };
    createTerminalView(host as unknown as HTMLElement, {
      onData: () => undefined,
      onCtrlC: () => undefined,
      onFontFit,
    });
    // The fonts promise is still pending. This geometry-less stub runs no
    // opening fit at all (runFit skips any host without dimensions), so the
    // callback firing is the only fit trigger in play.
    expect(onFontFit).not.toHaveBeenCalled();
    resolveFonts!();
    await new Promise((resolve) => setTimeout(resolve, 0));
    // Bundled JetBrains Mono can land after the first fit, changing the cell
    // while the box never moves. The view must not refit itself — only
    // TerminalSession.doResize sends session_resize — so the refit is routed
    // to the session's requestResize path.
    expect(onFontFit).toHaveBeenCalledTimes(1);
    // And the view ran no fit of its own for it (the stub host has no
    // geometry, so even the opening fit is skipped here).
    expect(mocks.state.fitCount).toBe(0);
  });

  it("does not fit on a collapsed or hidden host, and reports no fit to the PTY", () => {
    const host = { querySelector: () => null, clientWidth: 0, clientHeight: 0 };
    const view = createTerminalView(host as unknown as HTMLElement, {
      onData: () => undefined,
      onCtrlC: () => undefined,
    });
    const grid = { cols: mocks.state.terminals[0]!.cols, rows: mocks.state.terminals[0]!.rows };
    expect(view.fit()).toBe(false);
    expect(mocks.state.fitCount).toBe(0);
    expect(mocks.state.terminals[0]!.cols).toBe(grid.cols);
    expect(mocks.state.terminals[0]!.rows).toBe(grid.rows);
  });

  it("does not fit without FitAddon's proposal, and reports no fit to the PTY", () => {
    // The real addon returns undefined when it cannot propose (no element, or
    // zero cell metrics); the mock's unset proposal stands in for it.
    const host = { querySelector: () => null, clientWidth: 800, clientHeight: 384 };
    const view = createTerminalView(host as unknown as HTMLElement, {
      onData: () => undefined,
      onCtrlC: () => undefined,
    });
    expect(view.fit()).toBe(false);
    expect(mocks.state.fitCount).toBe(0);
    expect(mocks.state.terminals[0]!.cols).toBe(80);
    expect(mocks.state.terminals[0]!.rows).toBe(24);
  });

  it("the post-fit clamp treats FitAddon's proposal as the upper bound, scrollbar included", () => {
    // 80 columns of 10px fill the 800px box, but the addon reserved one
    // column for the scrollbar: the clamp must not add it back.
    const screen = { getBoundingClientRect: () => ({ width: 800, height: 384 }) };
    const host = { querySelector: () => screen, clientWidth: 800, clientHeight: 384 };
    const view = createTerminalView(host as unknown as HTMLElement, {
      onData: () => undefined,
      onCtrlC: () => undefined,
    });
    mocks.state.fitAddons[0]!.proposal = { cols: 79, rows: 24 };
    view.fit();
    expect(mocks.state.terminals[0]!.cols).toBe(79);
    expect(mocks.state.terminals[0]!.rows).toBe(24);
  });
});
