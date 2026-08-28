import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionSnapshot } from "../../types/ipc";
import type { TerminalViewHandle } from "./createTerminalView";
import {
  TerminalSession,
  type TerminalBanner,
  type TerminalChannel,
  type TerminalEvent,
  type TerminalSessionDeps,
} from "./terminalSession";
import type { TerminalSessionRecord, TerminalSessionRegistry } from "./terminalRegistry";

interface MockView extends TerminalViewHandle {
  written: string[];
  snapshots: SessionSnapshot[];
  fitCount: number;
  disposeCount: number;
  fitOk: { value: boolean };
  geometry: { cols: number; rows: number };
}

interface Harness {
  session: TerminalSession;
  view: MockView;
  invoke: ReturnType<typeof vi.fn>;
  emit: (event: TerminalEvent) => void;
  emitInput: (data: string) => void;
  flushFrame: () => void;
  completeSnapshot: () => void;
  resolveAttach: () => void;
  banners: TerminalBanner[];
  ctrlCStates: boolean[];
  registry: TerminalSessionRegistry;
}

function makeHarness(options?: {
  deferAttach?: boolean;
  deferSnapshot?: boolean;
  snapshotAsOfSeq?: number;
  existingSessionId?: string;
  missingFirstAttach?: boolean;
  rejectDetach?: boolean;
}): Harness {
  const written: string[] = [];
  const snapshots: SessionSnapshot[] = [];
  const fitOk = { value: true };
  const geometry = { cols: 80, rows: 24 };
  let fitCount = 0;
  let disposeCount = 0;
  let eventHandler: (event: TerminalEvent) => void = () => undefined;
  let inputHandler: (data: string) => void = () => undefined;
  let snapshotCallback: (() => void) | null = null;
  let frameCallback: (() => void) | null = null;
  const flushFrameNow = (): void => {
    const callback = frameCallback;
    frameCallback = null;
    callback?.();
  };
  let resolveAttach!: () => void;
  const attachGate = new Promise<void>((resolve) => {
    resolveAttach = resolve;
  });
  let registeredSessionId = options?.existingSessionId ?? null;
  const registry: TerminalSessionRegistry = {
    get: vi.fn((): TerminalSessionRecord | null =>
      registeredSessionId === null
        ? null
        : {
            workspaceId: "rust-core",
            sessionId: registeredSessionId,
            lastSeenSeq: null,
          },
    ),
    register: vi.fn((...args: [string | null, string]) => {
      registeredSessionId = args[1];
    }),
    updateCursor: vi.fn(),
    remove: vi.fn((...args: [string | null, string]) => {
      if (registeredSessionId === args[1]) registeredSessionId = null;
    }),
  };

  const view: MockView = {
    write: (data, callback) => {
      written.push(data);
      callback?.();
    },
    applySnapshot: (snapshot, callback) => {
      snapshots.push(snapshot);
      if (options?.deferSnapshot) {
        snapshotCallback = callback;
      } else {
        callback();
      }
    },
    fit: () => {
      fitCount += 1;
      return fitOk.value;
    },
    dispose: () => {
      disposeCount += 1;
    },
    cols: () => geometry.cols,
    rows: () => geometry.rows,
    written,
    snapshots,
    get fitCount() {
      return fitCount;
    },
    get disposeCount() {
      return disposeCount;
    },
    fitOk,
    geometry,
  };

  let attachCount = 0;
  const invoke = vi.fn(async (command: string) => {
    if (command === "sessions_list") {
      return [];
    }
    if (command === "session_create") {
      return {
        id: "session-1",
        workspaceId: "rust-core",
        kind: "terminal",
        title: "Terminal",
        state: { type: "live", generation: 1 },
      };
    }
    if (command === "session_attach" && options?.deferAttach) {
      await attachGate;
    }
    if (command === "session_attach") {
      attachCount += 1;
      if (options?.missingFirstAttach && attachCount === 1) {
        eventHandler({
          type: "snapshot",
          asOfSeq: 0,
          cols: 80,
          rows: 24,
          data: "",
          cursor: { row: 0, col: 0, visible: true, shape: "block", blinking: false },
          alternateScreen: false,
          bracketedPaste: false,
          lineWrap: true,
        });
        eventHandler({ type: "output", seq: 900, data: "old session" });
        flushFrameNow();
        throw "No session with that id.";
      }
      eventHandler({
        type: "snapshot",
        asOfSeq: options?.snapshotAsOfSeq ?? 0,
        cols: 80,
        rows: 24,
        data: "",
        cursor: { row: 0, col: 0, visible: true, shape: "block", blinking: false },
        alternateScreen: false,
        bracketedPaste: false,
        lineWrap: true,
      });
    }
    if (command === "session_detach" && options?.rejectDetach) {
      throw new Error("No session with that id.");
    }
    return undefined;
  });

  const channel = {
    onmessage: eventHandler,
  } as unknown as TerminalChannel;
  const deps: TerminalSessionDeps = {
    workspaceId: "rust-core",
    host: {} as HTMLElement,
    createView: async (_viewHost, options) => {
      inputHandler = options.onData;
      return view;
    },
    invoke: invoke as unknown as TerminalSessionDeps["invoke"],
    registry,
    createChannel: (handler) => {
      eventHandler = handler;
      Object.defineProperty(channel, "onmessage", {
        configurable: true,
        get: () => eventHandler,
        set: (nextHandler: (event: TerminalEvent) => void) => {
          eventHandler = nextHandler;
        },
      });
      return channel;
    },
    onBanner: (banner) => banners.push(banner),
    onCtrlCArmed: (armed) => ctrlCStates.push(armed),
    setTimeout: (callback, milliseconds) => setTimeout(callback, milliseconds) as unknown as number,
    clearTimeout: (id) => clearTimeout(id),
    scheduleFrame: (callback) => {
      frameCallback = callback;
      return 1;
    },
    cancelFrame: () => {
      frameCallback = null;
    },
  };
  const banners: TerminalBanner[] = [];
  const ctrlCStates: boolean[] = [];

  return {
    session: new TerminalSession(deps),
    view,
    invoke,
    emit: (event) => eventHandler(event),
    emitInput: (data) => inputHandler(data),
    flushFrame: flushFrameNow,
    completeSnapshot: () => {
      const callback = snapshotCallback;
      snapshotCallback = null;
      callback?.();
    },
    resolveAttach,
    banners,
    ctrlCStates,
    registry,
  };
}

const outputEvent = (seq: number, data: string): TerminalEvent => ({ type: "output", seq, data });
const exitEvent = (code: number | null): TerminalEvent => ({ type: "exit", code });

describe("TerminalSession startup and channel ordering", () => {
  it("creates a terminal and attaches with a fresh null cursor", async () => {
    const harness = makeHarness();
    await harness.session.start();

    expect(harness.invoke).toHaveBeenNthCalledWith(1, "sessions_list");
    expect(harness.invoke).toHaveBeenNthCalledWith(2, "session_create", {
      workspace_id: "rust-core",
      kind: "terminal",
    });
    expect(harness.invoke).toHaveBeenNthCalledWith(
      3,
      "session_attach",
      expect.objectContaining({
        id: "session-1",
        from_cursor: null,
        ch: expect.anything(),
      }),
    );
    expect(harness.registry.register).toHaveBeenCalledWith("rust-core", "session-1");
  });

  it("adopts the registered session without creating another shell", async () => {
    const harness = makeHarness({ existingSessionId: "existing-session" });
    await harness.session.start();

    expect(harness.invoke).not.toHaveBeenCalledWith("session_create", expect.anything());
    expect(harness.invoke).toHaveBeenCalledWith(
      "session_attach",
      expect.objectContaining({
        id: "existing-session",
        from_cursor: null,
        ch: expect.anything(),
      }),
    );
    expect(harness.registry.register).not.toHaveBeenCalled();
  });

  it("attaches a recovered transcript instead of creating a new shell", async () => {
    const harness = makeHarness();
    harness.invoke.mockImplementation(async (command: string) => {
      if (command === "sessions_list") {
        return [
          {
            id: "s.old.1",
            workspaceId: "rust-core",
            kind: "terminal",
            title: "Terminal",
            state: { type: "recovered", generation: 1, truncated: false },
          },
        ];
      }
      return undefined;
    });
    await harness.session.start();
    expect(harness.invoke).not.toHaveBeenCalledWith("session_create", expect.anything());
    expect(harness.invoke).toHaveBeenCalledWith(
      "session_attach",
      expect.objectContaining({
        id: "s.old.1",
        from_cursor: null,
      }),
    );
  });

  it("batches channel output into one xterm write per animation frame", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit(outputEvent(1, "one\n"));
    harness.emit(outputEvent(2, "two\n"));
    harness.emit(outputEvent(3, "three\n"));
    expect(harness.view.written).toEqual([]);

    harness.flushFrame();
    expect(harness.view.written).toEqual(["one\ntwo\nthree\n"]);
  });

  it("ignores a duplicate or stale sequence number", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit(outputEvent(4, "new"));
    harness.emit(outputEvent(4, "duplicate"));
    harness.emit(outputEvent(3, "stale"));
    harness.flushFrame();

    expect(harness.view.written).toEqual(["new"]);
  });

  it("uses the snapshot sequence boundary for output before and after restoration", async () => {
    const harness = makeHarness({ deferSnapshot: true, snapshotAsOfSeq: 2 });
    await harness.session.start();
    harness.invoke.mockClear();

    harness.emit(outputEvent(1, "covered-1"));
    harness.emit(outputEvent(2, "covered-2"));
    harness.emit(outputEvent(3, "live"));
    harness.emitInput("typed before snapshot");

    expect(harness.view.written).toEqual([]);
    expect(harness.invoke).not.toHaveBeenCalledWith("session_send", expect.anything());
    expect(harness.registry.updateCursor).not.toHaveBeenCalledWith("rust-core", "session-1", 2);

    harness.completeSnapshot();

    expect(harness.view.written).toEqual(["live"]);
    expect(harness.registry.updateCursor).toHaveBeenCalledWith("rust-core", "session-1", 2);
    expect(harness.invoke).toHaveBeenCalledWith("session_send", {
      id: "session-1",
      text: "typed before snapshot",
    });

    harness.emit(outputEvent(2, "covered-after-snapshot"));
    harness.flushFrame();
    expect(harness.view.written).toEqual(["live"]);
  });
});

describe("TerminalSession lifecycle and errors", () => {
  it("handles attach errors without closing the runtime-owned session", async () => {
    const harness = makeHarness();
    harness.invoke.mockImplementation(async (command: string) => {
      if (command === "sessions_list") return [];
      if (command === "session_create") {
        return {
          id: "session-1",
          workspaceId: "rust-core",
          kind: "terminal",
          title: "Terminal",
          state: { type: "live", generation: 1 },
        };
      }
      if (command === "session_attach") throw "No session with that id.";
      return undefined;
    });

    await harness.session.start();
    expect(harness.banners).toContainEqual({
      kind: "error",
      message: "Could not attach to the terminal: No session with that id.",
    });
    expect(harness.view.disposeCount).toBe(1);
    expect(harness.invoke).not.toHaveBeenCalledWith("session_close", expect.anything());
  });

  it("resets the sequence watermark when replacing a missing adopted session", async () => {
    const harness = makeHarness({ existingSessionId: "missing-session", missingFirstAttach: true });
    await harness.session.start();

    harness.emit(outputEvent(1, "new session"));
    harness.flushFrame();

    expect(harness.view.written).toEqual(["old session", "new session"]);
  });

  it("detaches without closing when disposed during an in-flight attach", async () => {
    const harness = makeHarness({ deferAttach: true });
    const startPromise = harness.session.start();
    await Promise.resolve();
    await Promise.resolve();

    harness.session.dispose();
    harness.resolveAttach();
    await startPromise;

    expect(
      harness.invoke.mock.calls.filter(([command]) => command === "session_close"),
    ).toHaveLength(0);
    expect(
      harness.invoke.mock.calls.filter(([command]) => command === "session_detach"),
    ).toHaveLength(1);
    expect(harness.invoke).toHaveBeenCalledWith("session_detach", { id: "session-1" });
    expect(harness.view.disposeCount).toBe(1);
  });

  it("marks exit once and ignores writes after exit", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit(exitEvent(7));
    harness.emit(exitEvent(7));
    harness.invoke.mockClear();
    await harness.session.writeToPty("ignored");

    expect(harness.banners).toContainEqual({ kind: "exited", code: 7 });
    expect(harness.registry.remove).toHaveBeenCalledWith("rust-core", "session-1");
    expect(harness.invoke).not.toHaveBeenCalled();
  });

  it("marks a recovered transcript without treating it as a live exit", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.emit(outputEvent(1, "scrollback"));
    harness.flushFrame();
    harness.emit({ type: "recovered", truncated: false });
    expect(harness.view.written).toEqual(["scrollback"]);
    expect(harness.banners).toContainEqual({ kind: "recovered", truncated: false });
    harness.invoke.mockClear();
    await harness.session.writeToPty("ignored");
    expect(harness.invoke).not.toHaveBeenCalled();
  });

  it("shows the incomplete scrollback banner while the session is still live", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit({ type: "journal_degraded" });
    harness.emit(outputEvent(1, "output after degradation"));
    harness.flushFrame();

    expect(harness.banners).toContainEqual({ kind: "journal_degraded" });
    expect(harness.view.written).toEqual(["output after degradation"]);
    expect(harness.registry.remove).not.toHaveBeenCalled();
  });

  it("declares an output gap and advances the reconnect cursor", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.emit({
      type: "output_gap",
      fromSeq: 2,
      toSeq: 4,
      droppedBytes: 30,
      droppedFrames: 3,
    });
    harness.emit(outputEvent(5, "after gap"));
    harness.flushFrame();

    expect(harness.banners).toContainEqual({ kind: "output_gap", fromSeq: 2, toSeq: 4 });
    expect(harness.registry.updateCursor).toHaveBeenCalledWith("rust-core", "session-1", 4);
    expect(harness.view.written).toEqual(["after gap"]);
  });

  it("surfaces unknown event types instead of treating them as output", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.emit({ type: "permission" } as unknown as TerminalEvent);
    harness.flushFrame();
    expect(harness.view.written).toEqual([]);
    expect(harness.banners).toContainEqual({
      kind: "error",
      message: "The daemon sent an unknown terminal event type: permission.",
    });
  });

  it("does not replace already-applied output when a stale snapshot arrives", async () => {
    const harness = makeHarness({ snapshotAsOfSeq: 1 });
    await harness.session.start();

    harness.emit(outputEvent(2, "already applied"));
    harness.flushFrame();
    harness.emit({
      type: "snapshot",
      asOfSeq: 1,
      cols: 80,
      rows: 24,
      data: "stale snapshot",
      cursor: { row: 0, col: 0, visible: true, shape: "block", blinking: false },
      alternateScreen: false,
      bracketedPaste: false,
      lineWrap: true,
    });

    expect(harness.view.snapshots).toHaveLength(1);
    expect(harness.view.written).toEqual(["already applied"]);
  });

  it("makes dispose idempotent and suppresses late output", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.session.dispose();
    harness.session.dispose();
    harness.emit(outputEvent(1, "late"));
    harness.flushFrame();

    expect(harness.view.disposeCount).toBe(1);
    expect(harness.view.written).toEqual([]);
    expect(
      harness.invoke.mock.calls.filter(([command]) => command === "session_close"),
    ).toHaveLength(0);
    expect(
      harness.invoke.mock.calls.filter(([command]) => command === "session_detach"),
    ).toHaveLength(1);
    expect(harness.invoke).toHaveBeenCalledWith("session_detach", { id: "session-1" });
  });

  it("still disposes and removes the listener when detach is rejected", async () => {
    const harness = makeHarness({ rejectDetach: true });
    await harness.session.start();

    harness.session.dispose();
    harness.emit(outputEvent(1, "late"));
    harness.flushFrame();

    expect(
      harness.invoke.mock.calls.filter(([command]) => command === "session_detach"),
    ).toHaveLength(1);
    expect(harness.view.disposeCount).toBe(1);
    expect(harness.view.written).toEqual([]);
    expect(harness.banners).not.toContainEqual({
      kind: "error",
      message: "No session with that id.",
    });
  });

  it("closes the session only when explicitly requested", async () => {
    const harness = makeHarness();
    await harness.session.start();

    harness.session.close();
    harness.session.close();

    expect(harness.registry.remove).toHaveBeenCalledWith("rust-core", "session-1");
    expect(
      harness.invoke.mock.calls.filter(([command]) => command === "session_close"),
    ).toHaveLength(1);
    expect(
      harness.invoke.mock.calls.filter(([command]) => command === "session_detach"),
    ).toHaveLength(0);
    expect(harness.view.disposeCount).toBe(1);
  });
});

describe("TerminalSession resize and Ctrl+C", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("resizes once at startup and coalesces subsequent changes", async () => {
    const harness = makeHarness();
    await harness.session.start();
    const initialFitCount = harness.view.fitCount;
    harness.invoke.mockClear();

    harness.session.requestResize();
    harness.session.requestResize();
    vi.advanceTimersByTime(149);
    expect(harness.view.fitCount).toBe(initialFitCount);

    vi.advanceTimersByTime(1);
    expect(harness.view.fitCount).toBe(initialFitCount + 1);
    expect(harness.invoke).toHaveBeenCalledTimes(1);
    expect(harness.invoke).toHaveBeenCalledWith("session_resize", {
      id: "session-1",
      cols: 80,
      rows: 24,
    });
  });

  it("does not report geometry when fit fails or dimensions are degenerate", async () => {
    const fitHarness = makeHarness();
    fitHarness.view.fitOk.value = false;
    await fitHarness.session.start();
    expect(fitHarness.invoke).not.toHaveBeenCalledWith("session_resize", expect.anything());

    const sizeHarness = makeHarness();
    sizeHarness.view.geometry.cols = 0;
    sizeHarness.view.geometry.rows = 0;
    await sizeHarness.session.start();
    expect(sizeHarness.invoke).not.toHaveBeenCalledWith("session_resize", expect.anything());
  });

  it("arms Ctrl+C first and sends ETX only on confirmation", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.invoke.mockClear();

    harness.session.requestCtrlC();
    expect(harness.ctrlCStates).toEqual([true]);
    expect(harness.invoke).not.toHaveBeenCalledWith("session_send", {
      id: "session-1",
      text: "\x03",
    });

    harness.session.requestCtrlC();
    await Promise.resolve();
    expect(harness.ctrlCStates).toEqual([true, false]);
    expect(harness.invoke).toHaveBeenCalledWith("session_send", { id: "session-1", text: "\x03" });
  });

  it("auto-disarms Ctrl+C without sending after three seconds", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.invoke.mockClear();

    harness.session.requestCtrlC();
    vi.advanceTimersByTime(3000);

    expect(harness.ctrlCStates).toEqual([true, false]);
    expect(harness.invoke).not.toHaveBeenCalledWith("session_send", expect.anything());
  });
});

describe("TerminalSession write failures", () => {
  it("catches backend errors and surfaces a banner after repeated failures", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.invoke.mockImplementation(async (command: string) => {
      if (command === "sessions_list") return [];
      if (command === "session_create") {
        return {
          id: "session-1",
          workspaceId: "rust-core",
          kind: "terminal",
          title: "Terminal",
          state: { type: "live", generation: 1 },
        };
      }
      if (command === "session_send") throw new Error("dead pipe");
      return undefined;
    });

    await harness.session.writeToPty("a");
    expect(harness.banners).not.toContainEqual({
      kind: "error",
      message: "Could not send input to the terminal.",
    });
    await harness.session.writeToPty("b");
    expect(harness.banners).toContainEqual({
      kind: "error",
      message: "Could not send input to the terminal.",
    });
  });
});
