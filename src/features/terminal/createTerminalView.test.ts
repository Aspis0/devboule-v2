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
  defaultProposal: FitProposal | null;
}

const mocks = vi.hoisted(() => {
  const state: TerminalMockState = {
    written: [],
    writeCallbacks: [],
    disposeCount: 0,
    fitCount: 0,
    terminals: [],
    fitAddons: [],
    defaultProposal: null,
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

    loadAddon(addon: MockFitAddon): void {
      addon.attach(this);
    }

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
    proposal: FitProposal | null = state.defaultProposal;
    private attached: MockTerminal | null = null;

    constructor() {
      state.fitAddons.push(this);
    }

    attach(terminal: MockTerminal): void {
      this.attached = terminal;
    }

    fit(): void {
      state.fitCount += 1;
      // A real FitAddon resizes its terminal to the proposal; the mock does
      // the same so a test can see which grid actually reaches the PTY.
      if (this.proposal !== null && this.attached !== null) {
        this.attached.cols = this.proposal.cols;
        this.attached.rows = this.proposal.rows;
      }
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
import {
  TerminalSession,
  type TerminalChannel,
  type TerminalEvent,
  type TerminalSessionDeps,
} from "./terminalSession";

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
      onFontsSettled: () => undefined,
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
      onFontsSettled: () => undefined,
    });
    const callback = vi.fn();

    view.applySnapshot(snapshot, callback);
    view.dispose();
    completeNextWrite();

    expect(callback).toHaveBeenCalledTimes(1);

    const secondView = createTerminalView({} as HTMLElement, {
      onData: () => undefined,
      onCtrlC: () => undefined,
      onFontsSettled: () => undefined,
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
    mocks.state.defaultProposal = null;
    vi.stubGlobal("getComputedStyle", () => ({ getPropertyValue: () => "0px" }));
    vi.stubGlobal("document", { documentElement: {}, fonts: { ready: Promise.resolve() } });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("tells the session the fonts settled instead of fitting on its own", async () => {
    const onFontsSettled = vi.fn();
    const host = { querySelector: () => null, clientWidth: 800, clientHeight: 384 };
    createTerminalView(host as unknown as HTMLElement, {
      onData: () => undefined,
      onCtrlC: () => undefined,
      onFontsSettled,
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    // A local refit here would leave xterm on a grid the PTY never hears
    // about: the session owns the fit that ends in a session_resize.
    expect(onFontsSettled).toHaveBeenCalledTimes(1);
  });

  it("the post-fit clamp treats FitAddon's proposal as the upper bound, scrollbar included", () => {
    // 80 columns of 10px fill the 800px box, but the addon reserved one
    // column for the scrollbar: the clamp must not add it back.
    const screen = { getBoundingClientRect: () => ({ width: 800, height: 384 }) };
    const host = { querySelector: () => screen, clientWidth: 800, clientHeight: 384 };
    const view = createTerminalView(host as unknown as HTMLElement, {
      onData: () => undefined,
      onCtrlC: () => undefined,
      onFontsSettled: () => undefined,
    });
    mocks.state.fitAddons[0]!.proposal = { cols: 79, rows: 24 };
    view.fit();
    expect(mocks.state.terminals[0]!.cols).toBe(79);
    expect(mocks.state.terminals[0]!.rows).toBe(24);
  });
});

interface SessionHarness {
  session: TerminalSession;
  invoke: ReturnType<typeof vi.fn>;
  /** Push the attach snapshot through the channel, as the daemon would. */
  settleSnapshot: () => void;
  /** Run the debounce timers the session queued, oldest first. */
  runTimers: () => void;
}

const handshakeSnapshot: SessionSnapshot = {
  type: "snapshot",
  asOfSeq: 0,
  cols: 90,
  rows: 20,
  data: "screen",
  cursor: { row: 0, col: 0, visible: true, shape: "block", blinking: false },
  alternateScreen: false,
  bracketedPaste: false,
  lineWrap: true,
};

function startSessionHandshake(host: HTMLElement): SessionHarness {
  const timers = new Map<number, () => void>();
  let nextTimerId = 1;
  let emitEvent: (event: TerminalEvent) => void = () => undefined;
  const invoke = vi.fn(async (command: string) => {
    if (command === "session_attach") return 17;
    return undefined;
  });
  const session = new TerminalSession({
    workspaceId: "rust-core",
    sessionId: "session-1",
    host,
    // The real view, so the test observes what the production wiring sends.
    createView: (viewHost, options) => Promise.resolve(createTerminalView(viewHost, options)),
    invoke: invoke as unknown as TerminalSessionDeps["invoke"],
    createChannel: (handler) => {
      emitEvent = handler;
      return { onmessage: handler } as unknown as TerminalChannel;
    },
    registry: {
      get: () => null,
      register: () => undefined,
      updateCursor: () => undefined,
      remove: () => undefined,
    },
    onBanner: () => undefined,
    onCtrlCArmed: () => undefined,
    setTimeout: (callback) => {
      const id = nextTimerId;
      nextTimerId += 1;
      timers.set(id, callback);
      return id;
    },
    clearTimeout: (id) => {
      timers.delete(id);
    },
    scheduleFrame: () => 0,
    cancelFrame: () => undefined,
  });
  return {
    session,
    invoke,
    settleSnapshot: () => emitEvent(handshakeSnapshot),
    runTimers: () => {
      const pending = [...timers.values()];
      timers.clear();
      for (const callback of pending) callback();
    },
  };
}

/** The session's start, its snapshot, and the debounced resize that follows. */
async function startAndSettle(harness: SessionHarness): Promise<void> {
  await harness.session.start();
  harness.settleSnapshot();
  completeNextWrite();
  completeNextWrite();
  harness.runTimers();
}

function resizeCalls(harness: SessionHarness): unknown[][] {
  return harness.invoke.mock.calls.filter(([command]) => command === "session_resize");
}

function stubFonts(ready: Promise<void>): void {
  vi.stubGlobal("document", { documentElement: {}, fonts: { ready } });
}

describe("the fit's handshake with the session (what reaches the PTY)", () => {
  beforeEach(() => {
    mocks.state.fitCount = 0;
    mocks.state.written.length = 0;
    mocks.state.writeCallbacks.length = 0;
    mocks.state.terminals.length = 0;
    mocks.state.fitAddons.length = 0;
    mocks.state.defaultProposal = null;
    vi.stubGlobal("getComputedStyle", () => ({ getPropertyValue: () => "0px" }));
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("hands the font-settle refit to the session, so the PTY receives the new grid", async () => {
    mocks.state.defaultProposal = { cols: 80, rows: 24 };
    let settleFonts!: () => void;
    const fontsReady = new Promise<void>((resolve) => {
      settleFonts = resolve;
    });
    stubFonts(fontsReady);
    const host = { querySelector: () => null, clientWidth: 800, clientHeight: 384 };
    const harness = startSessionHandshake(host as unknown as HTMLElement);

    await startAndSettle(harness);
    expect(resizeCalls(harness)).toEqual([
      ["session_resize", { id: "session-1", subscriptionId: 17, cols: 80, rows: 24 }],
    ]);

    // JetBrains Mono lands: the cell changes shape while the host box does not.
    mocks.state.fitAddons[0]!.proposal = { cols: 100, rows: 30 };
    settleFonts();
    await new Promise((resolve) => setTimeout(resolve, 0));
    harness.runTimers();

    expect(resizeCalls(harness)).toEqual([
      ["session_resize", { id: "session-1", subscriptionId: 17, cols: 80, rows: 24 }],
      ["session_resize", { id: "session-1", subscriptionId: 17, cols: 100, rows: 30 }],
    ]);
    harness.session.dispose();
  });

  it("never fits a collapsed host, so the PTY keeps the last good grid", async () => {
    // FitAddon proposes its 2-column/1-row floor for a zero box: a positive
    // grid that would be sent as a real resize if the host were fitted.
    mocks.state.defaultProposal = { cols: 2, rows: 1 };
    stubFonts(Promise.resolve());
    const host = { querySelector: () => null, clientWidth: 0, clientHeight: 0 };
    const harness = startSessionHandshake(host as unknown as HTMLElement);

    await startAndSettle(harness);

    expect(mocks.state.fitCount).toBe(0);
    expect(resizeCalls(harness)).toEqual([]);
    // The snapshot's grid, not the 2x1 the addon proposed for the zero box.
    expect(mocks.state.terminals[0]!.cols).toBe(90);
    expect(mocks.state.terminals[0]!.rows).toBe(20);
    harness.session.dispose();
  });

  it("sends nothing when FitAddon has no proposal", async () => {
    // A zero renderer cell width or height is what makes the addon return
    // no proposal; without one there is no bounded grid to fit to.
    mocks.state.defaultProposal = null;
    stubFonts(Promise.resolve());
    const host = { querySelector: () => null, clientWidth: 800, clientHeight: 384 };
    const harness = startSessionHandshake(host as unknown as HTMLElement);

    await startAndSettle(harness);

    expect(mocks.state.fitCount).toBe(0);
    expect(resizeCalls(harness)).toEqual([]);
    expect(mocks.state.terminals[0]!.cols).toBe(90);
    harness.session.dispose();
  });

  it("leaves the addon's grid alone when the measured cell has no height", async () => {
    mocks.state.defaultProposal = { cols: 100, rows: 20 };
    stubFonts(Promise.resolve());
    // The screen reports ten times the host's width and no height at all: the
    // rows arithmetic would divide by zero, and a columns figure measured off
    // that screen would shrink the grid on a measurement nothing supports.
    const screen = { getBoundingClientRect: () => ({ width: 8000, height: 0 }) };
    const host = { querySelector: () => screen, clientWidth: 800, clientHeight: 384 };
    const harness = startSessionHandshake(host as unknown as HTMLElement);

    await startAndSettle(harness);

    expect(resizeCalls(harness)).toEqual([
      ["session_resize", { id: "session-1", subscriptionId: 17, cols: 100, rows: 20 }],
    ]);
    harness.session.dispose();
  });
});
