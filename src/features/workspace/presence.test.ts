import { beforeEach, describe, expect, it, vi } from "vitest";
import { reportSelection, startPresenceReporting, type PresenceDeps } from "./presence";
import type { CommandArgs } from "../../lib/tauri";
import type { WindowState } from "./attentionNotice";

/** Lets an async emit settle before the assertions read the sends. */
async function flush(): Promise<void> {
  for (let i = 0; i < 6; i += 1) await Promise.resolve();
}

type Listener = () => void;

/**
 * Fake window/document pair. Events are fired explicitly so the tests stay
 * deterministic; the visibility and focus answers are read live from `state`,
 * exactly like the real DOM reads them at listener time.
 */
function createEnvironment(overrides: { visibilityState?: string; hasFocus?: boolean } = {}) {
  const listeners = new Map<string, Set<Listener>>();
  const state = {
    visibilityState: overrides.visibilityState ?? "visible",
    hasFocus: overrides.hasFocus ?? true,
  };
  const invoke = vi.fn(async () => undefined);

  const attach = (target: "window" | "document") => ({
    addEventListener: (type: string, listener: Listener) => {
      const key = `${target}:${type}`;
      const set = listeners.get(key) ?? new Set<Listener>();
      set.add(listener);
      listeners.set(key, set);
    },
    removeEventListener: (type: string, listener: Listener) => {
      listeners.get(`${target}:${type}`)?.delete(listener);
    },
  });

  return {
    invoke,
    state,
    window: attach("window"),
    document: {
      ...attach("document"),
      get visibilityState() {
        return state.visibilityState;
      },
      hasFocus: () => state.hasFocus,
    },
    fire: (type: string, target: "window" | "document" = "window") => {
      for (const listener of listeners.get(`${target}:${type}`) ?? []) listener();
    },
  };
}

/**
 * The raw vi.fn double is not assignable to the generic `invoke` dep, so the
 * cast lives here (same pattern as agentSession.test.ts) and every test starts
 * its reporter through this helper.
 */
function createReporter(env: ReturnType<typeof createEnvironment>) {
  return startPresenceReporting({
    invoke: env.invoke as unknown as PresenceDeps["invoke"],
    window: env.window,
    document: env.document,
  });
}

describe("presence reporting", () => {
  // The reported selection is module state — the one record the reporter and
  // the toast gate share — so every test starts from "no surface shows a
  // session" rather than from whatever the previous test left behind.
  beforeEach(() => {
    reportSelection(null);
  });

  it("reports presence once on startup so the daemon is not guessing", () => {
    const env = createEnvironment();
    createReporter(env);

    expect(env.invoke).toHaveBeenCalledTimes(1);
    expect(env.invoke).toHaveBeenCalledWith("session_presence", {
      focusedSessionId: null,
      appVisible: true,
    });
  });

  it("a reporter starting after a selection reports the stored selection, and null when it is withdrawn", async () => {
    // React flushes a commit's effects child-first: the surface's
    // reportSelection can land before any reporter exists. The write is
    // stored, so the reporter picks it up at start instead of guessing null
    // — and a withdrawal (the surface unmounted) is reported as null.
    const env = createEnvironment();
    reportSelection("session-a");
    const reporter = startPresenceReporting({
      invoke: env.invoke as unknown as PresenceDeps["invoke"],
      window: env.window,
      document: env.document,
    });
    await flush();
    expect(env.invoke).toHaveBeenCalledWith("session_presence", {
      focusedSessionId: "session-a",
      appVisible: true,
    });

    reportSelection(null);
    await flush();
    expect(env.invoke).toHaveBeenLastCalledWith("session_presence", {
      focusedSessionId: null,
      appVisible: true,
    });
    reporter.dispose();
  });

  it("sends the focused session when the selection changes", async () => {
    const env = createEnvironment();
    createReporter(env);

    reportSelection("session-a");
    await flush();

    expect(env.invoke).toHaveBeenNthCalledWith(2, "session_presence", {
      focusedSessionId: "session-a",
      appVisible: true,
    });
  });

  it("reports the app as unfocused on blur and focused again on focus", async () => {
    const env = createEnvironment();
    createReporter(env);
    reportSelection("session-a");
    await flush();

    env.state.hasFocus = false;
    env.fire("blur");
    await flush();
    expect(env.invoke).toHaveBeenNthCalledWith(3, "session_presence", {
      focusedSessionId: null,
      appVisible: false,
    });

    env.state.hasFocus = true;
    env.fire("focus");
    await flush();
    expect(env.invoke).toHaveBeenNthCalledWith(4, "session_presence", {
      focusedSessionId: "session-a",
      appVisible: true,
    });
  });

  it("reports hidden on visibilitychange and restores when visible again", async () => {
    const env = createEnvironment();
    createReporter(env);
    reportSelection("session-a");
    await flush();

    env.state.visibilityState = "hidden";
    env.fire("visibilitychange", "document");
    await flush();
    expect(env.invoke).toHaveBeenNthCalledWith(3, "session_presence", {
      focusedSessionId: null,
      appVisible: false,
    });

    env.state.visibilityState = "visible";
    env.fire("visibilitychange", "document");
    await flush();
    expect(env.invoke).toHaveBeenNthCalledWith(4, "session_presence", {
      focusedSessionId: "session-a",
      appVisible: true,
    });
  });

  it("does not re-send presence when nothing changed", async () => {
    const env = createEnvironment();
    createReporter(env);
    reportSelection("session-a");
    await flush();
    const callsAfterSettling = env.invoke.mock.calls.length;

    // Repeating the same selection and firing events that do not change the
    // real visibility/focus answers must not produce any new sends.
    reportSelection("session-a");
    env.fire("focus");
    env.fire("visibilitychange", "document");
    await flush();

    expect(env.invoke).toHaveBeenCalledTimes(callsAfterSettling);

    // A real change produces exactly one new send.
    env.state.hasFocus = false;
    env.fire("blur");
    await flush();
    expect(env.invoke).toHaveBeenCalledTimes(callsAfterSettling + 1);
  });

  it("asks the window state, so a hidden window is reported hidden even while the page lies", async () => {
    // The live check measured this: hidden in the tray, the document still
    // claims visible and focused. The window state is the truth.
    const env = createEnvironment({ visibilityState: "visible", hasFocus: true });
    const windowState = async (): Promise<WindowState> => ({
      visible: false,
      focused: false,
      minimized: false,
    });
    const reporter = startPresenceReporting({
      invoke: env.invoke as unknown as PresenceDeps["invoke"],
      window: env.window,
      document: env.document,
      windowState,
    });

    await flush();
    expect(env.invoke).toHaveBeenCalledWith("session_presence", {
      focusedSessionId: null,
      appVisible: false,
    });
    reporter.dispose();
  });

  it("reports the window state as visible and focused when it really is", async () => {
    const env = createEnvironment({ visibilityState: "visible", hasFocus: true });
    const windowState = async (): Promise<WindowState> => ({
      visible: true,
      focused: true,
      minimized: false,
    });
    const reporter = startPresenceReporting({
      invoke: env.invoke as unknown as PresenceDeps["invoke"],
      window: env.window,
      document: env.document,
      windowState,
    });

    await flush();
    expect(env.invoke).toHaveBeenCalledWith("session_presence", {
      focusedSessionId: null,
      appVisible: true,
    });
    reporter.dispose();
  });

  it("reports not visible when the window-state read rejects, never the page's lie", async () => {
    const env = createEnvironment({ visibilityState: "visible", hasFocus: true });
    const windowState = async (): Promise<WindowState> => {
      throw new Error("the window could not be asked");
    };
    const reporter = startPresenceReporting({
      invoke: env.invoke as unknown as PresenceDeps["invoke"],
      window: env.window,
      document: env.document,
      windowState,
    });

    await flush();
    expect(env.invoke).toHaveBeenCalledWith("session_presence", {
      focusedSessionId: null,
      appVisible: false,
    });
    reporter.dispose();
  });

  it("never reports after dispose, even with a state read still in flight", async () => {
    const env = createEnvironment({ visibilityState: "visible", hasFocus: true });
    let resolveState: (state: WindowState) => void = () => undefined;
    const windowState = (): Promise<WindowState> =>
      new Promise((resolve) => {
        resolveState = resolve;
      });
    const reporter = startPresenceReporting({
      invoke: env.invoke as unknown as PresenceDeps["invoke"],
      window: env.window,
      document: env.document,
      windowState,
    });
    reportSelection("session-a");
    reporter.dispose();

    // The read was still pending when dispose landed; resolving it now must
    // not conjure a late report that re-asserts an attended session.
    resolveState({ visible: true, focused: true, minimized: false });
    await flush();
    expect(env.invoke).not.toHaveBeenCalled();
  });

  it("reports at once on a window focus change, with the poll only as a net", async () => {
    vi.useFakeTimers();
    const env = createEnvironment({ visibilityState: "visible", hasFocus: true });
    let hidden = false;
    // A holder, not a bare `let`: the assignment happens inside the
    // subscription callback, which TypeScript's narrowing cannot see.
    const focusHandlerHolder: {
      handler: ((event: { payload: boolean }) => void) | null;
    } = { handler: null };
    const reporter = startPresenceReporting({
      invoke: env.invoke as unknown as PresenceDeps["invoke"],
      window: env.window,
      document: env.document,
      windowState: async (): Promise<WindowState> => ({
        visible: !hidden,
        focused: !hidden,
        minimized: false,
      }),
      onWindowFocusChange: (handler) => {
        focusHandlerHolder.handler = handler;
        return Promise.resolve(() => {
          focusHandlerHolder.handler = null;
        });
      },
    });
    await flush();

    // The window hides: no DOM event fires inside WebView2, but the focus
    // change subscription re-asks the state at once.
    hidden = true;
    focusHandlerHolder.handler?.({ payload: false });
    await flush();
    expect(env.invoke).toHaveBeenCalledWith("session_presence", {
      focusedSessionId: null,
      appVisible: false,
    });
    reporter.dispose();
    vi.useRealTimers();
  });

  it("an older read never overwrites a newer one", async () => {
    const env = createEnvironment({ visibilityState: "visible", hasFocus: true });
    const reads: Array<(state: WindowState) => void> = [];
    const reporter = startPresenceReporting({
      invoke: env.invoke as unknown as PresenceDeps["invoke"],
      window: env.window,
      document: env.document,
      windowState: (): Promise<WindowState> =>
        new Promise((resolve) => {
          reads.push(resolve);
        }),
    });
    await flush();
    expect(reads.length).toBe(1);
    // The window hides while the first read is still in flight; a second
    // read starts (the selection change re-emits).
    reportSelection("session-a");
    expect(reads.length).toBe(2);
    // The newer read answers hidden first.
    reads[1]({ visible: false, focused: false, minimized: false });
    await flush();
    expect(env.invoke).toHaveBeenLastCalledWith("session_presence", {
      focusedSessionId: null,
      appVisible: false,
    });
    // The older read answers visible and focused, late: it is dropped (it
    // was the startup read), so the daemon keeps the hidden answer.
    reads[0]({ visible: true, focused: true, minimized: false });
    await flush();
    expect(env.invoke).toHaveBeenCalledTimes(1);
    expect(env.invoke).toHaveBeenLastCalledWith("session_presence", {
      focusedSessionId: null,
      appVisible: false,
    });
    reporter.dispose();
  });

  it("removes the subscription when dispose lands before setup resolves", async () => {
    const env = createEnvironment();
    const unlisten = vi.fn();
    let resolveSubscription: (un: () => void) => void = () => undefined;
    const reporter = startPresenceReporting({
      invoke: env.invoke as unknown as PresenceDeps["invoke"],
      window: env.window,
      document: env.document,
      onWindowFocusChange: () =>
        new Promise((resolve) => {
          resolveSubscription = resolve;
        }),
    });
    // Cleanup lands while the subscription setup is still pending.
    reporter.dispose();
    resolveSubscription(unlisten);
    await flush();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("sends the same presence again after a failed send", async () => {
    // A rejected session_presence must not be swallowed by the dedupe: the
    // next event sends the same pair again, or the daemon keeps stale
    // attended presence forever.
    const env = createEnvironment({ visibilityState: "visible", hasFocus: true });
    let fail = true;
    const sent: Array<CommandArgs["session_presence"]> = [];
    const invoke = vi.fn((_command: string, args: CommandArgs["session_presence"]) => {
      sent.push(args);
      if (fail) return Promise.reject(new Error("the pipe is gone"));
      return Promise.resolve();
    });
    const reporter = startPresenceReporting({
      invoke: invoke as unknown as PresenceDeps["invoke"],
      window: env.window,
      document: env.document,
      windowState: async (): Promise<WindowState> => ({
        visible: true,
        focused: true,
        minimized: false,
      }),
    });
    await flush();
    expect(sent.length).toBe(1);
    fail = false;
    // The same pair again — an event fires, the failed send must be retried.
    env.fire("focus");
    await flush();
    expect(sent.length).toBe(2);
    expect(sent[1]).toEqual({ focusedSessionId: null, appVisible: true });
    reporter.dispose();
  });

  it("waits for a presence write to settle before starting the next one", async () => {
    const env = createEnvironment({ visibilityState: "visible", hasFocus: true });
    const sent: Array<CommandArgs["session_presence"]> = [];
    let hidden = false;
    let readsApplied = 0;
    let resolveFirst: (state: WindowState) => void = () => undefined;
    const invoke = vi.fn((_command: string, args: CommandArgs["session_presence"]) => {
      sent.push(args);
      if (sent.length === 1) {
        return new Promise((resolve) => {
          resolveFirst = resolve;
        });
      }
      return Promise.resolve();
    });
    const reporter = startPresenceReporting({
      invoke: invoke as unknown as PresenceDeps["invoke"],
      window: env.window,
      document: env.document,
      windowState: async (): Promise<WindowState> => {
        readsApplied += 1;
        return { visible: !hidden, focused: !hidden, minimized: false };
      },
    });
    await flush();
    expect(sent.length).toBe(1);
    // The window hides while the first write is still unsettled. The second
    // read applies, but the second INVOKE must not start while the first
    // write is pending.
    hidden = true;
    env.fire("visibilitychange", "document");
    await flush();
    expect(readsApplied).toBe(2);
    expect(sent.length).toBe(1);
    resolveFirst({ visible: true, focused: true, minimized: false });
    await flush();
    expect(sent.length).toBe(2);
    expect(sent[1]).toEqual({ focusedSessionId: null, appVisible: false });
    reporter.dispose();
  });

  it("drops the queued answer when dispose lands while a send is in flight", async () => {
    const env = createEnvironment({ visibilityState: "visible", hasFocus: true });
    const sent: Array<CommandArgs["session_presence"]> = [];
    let hidden = false;
    let readsApplied = 0;
    let resolveFirst: (state: WindowState) => void = () => undefined;
    const invoke = vi.fn((_command: string, args: CommandArgs["session_presence"]) => {
      sent.push(args);
      if (sent.length === 1) {
        return new Promise((resolve) => {
          resolveFirst = resolve;
        });
      }
      return Promise.resolve();
    });
    const reporter = startPresenceReporting({
      invoke: invoke as unknown as PresenceDeps["invoke"],
      window: env.window,
      document: env.document,
      windowState: async (): Promise<WindowState> => {
        readsApplied += 1;
        return { visible: !hidden, focused: !hidden, minimized: false };
      },
    });
    await flush();
    expect(sent.length).toBe(1);
    // B queues behind the in-flight A, and dispose lands before A settles.
    hidden = true;
    env.fire("visibilitychange", "document");
    await flush();
    expect(readsApplied).toBe(2);
    reporter.dispose();
    // Settling A must NOT send the queued B after disposal.
    resolveFirst({ visible: true, focused: true, minimized: false });
    await flush();
    expect(sent.length).toBe(1);
  });

  it("stops listening and sending after dispose", () => {
    const env = createEnvironment();
    const reporter = createReporter(env);
    reporter.dispose();

    env.state.hasFocus = false;
    env.fire("blur");
    env.state.visibilityState = "hidden";
    env.fire("visibilitychange", "document");
    reportSelection("session-a");

    // Only the startup send remains.
    expect(env.invoke).toHaveBeenCalledTimes(1);
  });
});
