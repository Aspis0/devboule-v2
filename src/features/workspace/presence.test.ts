import { describe, expect, it, vi } from "vitest";
import { startPresenceReporting, type PresenceDeps } from "./presence";

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
