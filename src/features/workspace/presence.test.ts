import { describe, expect, it, vi } from "vitest";
import { startPresenceReporting, type PresenceDeps } from "./presence";
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
  it("reports presence once on startup so the daemon is not guessing", () => {
    const env = createEnvironment();
    createReporter(env);

    expect(env.invoke).toHaveBeenCalledTimes(1);
    expect(env.invoke).toHaveBeenCalledWith("session_presence", {
      focusedSessionId: null,
      appVisible: true,
    });
  });

  it("sends the focused session when the selection changes", () => {
    const env = createEnvironment();
    const reporter = createReporter(env);

    reporter.onSelectionChanged("session-a");

    expect(env.invoke).toHaveBeenNthCalledWith(2, "session_presence", {
      focusedSessionId: "session-a",
      appVisible: true,
    });
  });

  it("reports the app as unfocused on blur and focused again on focus", () => {
    const env = createEnvironment();
    const reporter = createReporter(env);
    reporter.onSelectionChanged("session-a");

    env.state.hasFocus = false;
    env.fire("blur");
    expect(env.invoke).toHaveBeenNthCalledWith(3, "session_presence", {
      focusedSessionId: null,
      appVisible: false,
    });

    env.state.hasFocus = true;
    env.fire("focus");
    expect(env.invoke).toHaveBeenNthCalledWith(4, "session_presence", {
      focusedSessionId: "session-a",
      appVisible: true,
    });
  });

  it("reports hidden on visibilitychange and restores when visible again", () => {
    const env = createEnvironment();
    const reporter = createReporter(env);
    reporter.onSelectionChanged("session-a");

    env.state.visibilityState = "hidden";
    env.fire("visibilitychange", "document");
    expect(env.invoke).toHaveBeenNthCalledWith(3, "session_presence", {
      focusedSessionId: null,
      appVisible: false,
    });

    env.state.visibilityState = "visible";
    env.fire("visibilitychange", "document");
    expect(env.invoke).toHaveBeenNthCalledWith(4, "session_presence", {
      focusedSessionId: "session-a",
      appVisible: true,
    });
  });

  it("does not re-send presence when nothing changed", () => {
    const env = createEnvironment();
    const reporter = createReporter(env);
    reporter.onSelectionChanged("session-a");
    const callsAfterSettling = env.invoke.mock.calls.length;

    // Repeating the same selection and firing events that do not change the
    // real visibility/focus answers must not produce any new sends.
    reporter.onSelectionChanged("session-a");
    env.fire("focus");
    env.fire("visibilitychange", "document");

    expect(env.invoke).toHaveBeenCalledTimes(callsAfterSettling);

    // A real change produces exactly one new send.
    env.state.hasFocus = false;
    env.fire("blur");
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
    reporter.onSelectionChanged("session-a");
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
    const focusHandlerHolder: { handler: (() => void) | null } = { handler: null };
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
        return () => {
          focusHandlerHolder.handler = null;
        };
      },
    });
    await flush();

    // The window hides: no DOM event fires inside WebView2, but the focus
    // change subscription re-asks the state at once.
    hidden = true;
    focusHandlerHolder.handler?.();
    await flush();
    expect(env.invoke).toHaveBeenCalledWith("session_presence", {
      focusedSessionId: null,
      appVisible: false,
    });
    reporter.dispose();
    vi.useRealTimers();
  });

  it("stops listening and sending after dispose", () => {
    const env = createEnvironment();
    const reporter = createReporter(env);
    reporter.dispose();

    env.state.hasFocus = false;
    env.fire("blur");
    env.state.visibilityState = "hidden";
    env.fire("visibilitychange", "document");
    reporter.onSelectionChanged("session-a");

    // Only the startup send remains.
    expect(env.invoke).toHaveBeenCalledTimes(1);
  });
});
