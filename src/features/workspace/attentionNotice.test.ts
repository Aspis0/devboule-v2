// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Attention, AttentionReason, Session, SessionStateSnapshot } from "../../types/ipc";
import type { ToastContent, ToastDeps, WindowState } from "./attentionNotice";
import {
  PREVIEW_LIMIT,
  TOAST_RETRY_DELAY_MS,
  attentionRaised,
  fireAttentionToast,
  forgetAttentionFor,
  heldContentForSession,
  previewFrom,
  sendWithPermission,
  setAttentionHeldContentProvider,
  stripMarkdown,
  toastContent,
  toastGate,
  workspaceHeldContentProvider,
} from "./attentionNotice";
import {
  lookedAtSessionId,
  reportSelection,
  startPresenceReporting,
  type PresenceDeps,
} from "./presence";
import { createWorkspaceSessionController, sessionTitle } from "./workspaceSessions";

// The live check measured this lie: with the window hidden in the tray the
// document inside WebView2 still claims visible and focused. Every gate
// test below mocks the document into this lying state on purpose.
function documentClaimsVisibleAndFocused(): void {
  Object.defineProperty(document, "visibilityState", {
    value: "visible",
    configurable: true,
  });
  vi.spyOn(document, "hasFocus").mockReturnValue(true);
}

const windowStateOf =
  (state: Partial<WindowState>): (() => Promise<WindowState>) =>
  async () => ({
    visible: false,
    focused: false,
    minimized: false,
    ...state,
  });

const hiddenInTray = windowStateOf({ visible: false, focused: false, minimized: false });
const onScreenFocused = windowStateOf({ visible: true, focused: true, minimized: false });
const minimized = windowStateOf({ visible: true, focused: true, minimized: true });

function attention(reason: Attention["reason"], atMs: number): Attention {
  return { reason, atMs };
}

/**
 * Deferred window-state reads, resolved per fire: the only way to test
 * raises that pause inside their own window-state read.
 */
function deferredWindowState(): {
  windowState: () => Promise<WindowState>;
  resolveNext: (state: WindowState) => void;
} {
  const resolvers: Array<(state: WindowState) => void> = [];
  return {
    windowState: () =>
      new Promise<WindowState>((resolve) => {
        resolvers.push(resolve);
      }),
    resolveNext: (state) => resolvers.shift()?.(state),
  };
}

describe("attentionRaised", () => {
  it("fires exactly once per raise", () => {
    const raise = attention("finished", 1000);
    expect(attentionRaised(undefined, raise)).toBe(true);
    // Harmless roster re-publications of the same raise: silence.
    expect(attentionRaised(raise, raise)).toBe(false);
    expect(attentionRaised(attention("finished", 1000), raise)).toBe(false);
  });

  it("does not fire when attention clears or stays cleared", () => {
    expect(attentionRaised(attention("error", 1000), undefined)).toBe(false);
    expect(attentionRaised(undefined, undefined)).toBe(false);
  });

  it("fires again only for a newer raise, never an older push", () => {
    const first = attention("permission", 1000);
    const second = attention("finished", 2000);
    expect(attentionRaised(first, second)).toBe(true);
    // A late push carrying the older event is not a new event.
    expect(attentionRaised(second, first)).toBe(false);
  });

  it("treats a same-millisecond reason change as a new raise", () => {
    // The daemon can replace a lower-priority raise with a higher-priority
    // one in the same millisecond: the escalation is new news.
    const first = attention("finished", 1000);
    const escalation = attention("permission", 1000);
    expect(attentionRaised(first, escalation)).toBe(true);
    expect(attentionRaised(undefined, escalation)).toBe(true);
  });

  it("treats a reason the priority table does not know as new", () => {
    // The TypeScript union does not validate the incoming IPC payload: a
    // future daemon reason arrives as a string the table lacks. Unknown is
    // not "lower" — at an equal stamp it is announced.
    const known = attention("permission", 1000);
    const unknown = { reason: "quantum" as AttentionReason, atMs: 1000 };
    expect(attentionRaised(known, unknown)).toBe(true);
  });

  it("only a higher-priority reason is new at an equal timestamp", () => {
    // finished < error < permission, the daemon's own order
    // (AttentionReason::priority). A delayed lower-priority push at the
    // same stamp is a stale duplicate, not a downgrade announcement.
    expect(attentionRaised(attention("finished", 1000), attention("error", 1000))).toBe(true);
    expect(attentionRaised(attention("error", 1000), attention("permission", 1000))).toBe(true);
    expect(attentionRaised(attention("permission", 1000), attention("finished", 1000))).toBe(false);
    expect(attentionRaised(attention("permission", 1000), attention("error", 1000))).toBe(false);
    expect(attentionRaised(attention("permission", 1000), attention("permission", 1000))).toBe(
      false,
    );
  });
});

describe("toastGate", () => {
  // Paseo's rule: away from the WINDOW or away from the SESSION — the raise
  // is silent only when the user is looking at the session that raised.
  const seen = { visible: true, focused: true, minimized: false };

  it("stays silent only for the raise of the session this window shows", () => {
    expect(toastGate(seen, "a1", "a1")).toBe(false);
  });

  it("announces another session's raise while this window stays focused", () => {
    // The case the window-level gate lost after navigation scoped the strip
    // to one workspace: the raised tab is not on screen, and nobody looks at
    // every tab at once.
    expect(toastGate(seen, "a2", "a1")).toBe(true);
    // No surface shows a session at all (Workspace unmounted, Settings).
    expect(toastGate(seen, "a2", null)).toBe(true);
  });

  it("announces any raise when the window is hidden, minimized, or behind", () => {
    expect(toastGate({ visible: false, focused: false, minimized: false }, "a1", "a1")).toBe(true);
    expect(toastGate({ visible: true, focused: true, minimized: true }, "a1", "a1")).toBe(true);
    expect(toastGate({ visible: true, focused: false, minimized: false }, "a1", "a1")).toBe(true);
  });
});

describe("lookedAtSessionId, the gate's other half", () => {
  // The gate and the daemon's presence report read ONE record: whatever the
  // Workspace surface last reported, and nothing after it unmounts.
  it("is the session the surface reported, until the surface withdraws it", () => {
    reportSelection("a1");
    expect(lookedAtSessionId()).toBe("a1");
    reportSelection(null);
    expect(lookedAtSessionId()).toBeNull();
  });
});

describe("stripMarkdown", () => {
  it("keeps plain text untouched", () => {
    expect(stripMarkdown("Build passed in 3m 20s")).toBe("Build passed in 3m 20s");
  });

  it("reduces the constructs to their words", () => {
    expect(stripMarkdown("## Status")).toBe("Status");
    expect(stripMarkdown("see [the docs](https://example.com) now")).toBe("see the docs now");
    expect(stripMarkdown("**done** and *fast* and `fixed`")).toBe("done and fast and fixed");
    expect(stripMarkdown("- one\n- two")).toBe("one\ntwo");
  });

  it("collapses whitespace runs", () => {
    expect(stripMarkdown("a   b\n\n  c")).toBe("a b\nc");
  });
});

describe("previewFrom", () => {
  it("caps the preview around 220 characters on a boundary", () => {
    const long = "word ".repeat(200).trim();
    const preview = previewFrom(long);
    expect(preview.length).toBeLessThanOrEqual(PREVIEW_LIMIT + 1);
    expect(preview.endsWith("…")).toBe(true);
    expect(long.startsWith(preview.slice(0, -1))).toBe(true);
  });

  it("leaves short text alone", () => {
    expect(previewFrom("All checks passed")).toBe("All checks passed");
  });
});

describe("toastContent", () => {
  it("names the session and the reason in the title", () => {
    expect(toastContent("fix login", "permission", undefined).title).toContain("fix login");
    expect(toastContent("fix login", "permission", undefined).title).toContain("needs approval");
  });

  it("carries the held permission request in the body", () => {
    const content = toastContent("fix login", "permission", {
      permissionText: "Run npm install",
    });
    expect(content.body).toBe("Run npm install");
  });

  it("carries the held assistant preview for finished", () => {
    const content = toastContent("fix login", "finished", {
      lastAssistantText: "Deploy finished successfully",
    });
    expect(content.body).toContain("Deploy finished successfully");
  });

  it("falls back to the bare reason when nothing is held", () => {
    expect(toastContent("fix login", "permission", undefined).body).toBe("needs approval");
    expect(toastContent("fix login", "finished", {}).body).toBe("finished");
    expect(toastContent("fix login", "error", undefined).body).toBe("error");
  });

  it("never invents content it does not hold", () => {
    const content = toastContent("fix login", "finished", undefined);
    expect(content.body).toBe("finished");
    expect(content.body.length).toBeLessThan(20);
  });
});

describe("heldContentForSession", () => {
  it("hands over the pending card's text for a session this window sees", () => {
    const held = heldContentForSession(true, { title: "Run npm install" });
    expect(held?.permissionText).toContain("Run npm install");
  });

  it("keeps the description beside the title", () => {
    const held = heldContentForSession(true, {
      title: "Run npm install",
      description: "the agent wants to install a package",
    });
    expect(held?.permissionText).toContain("Run npm install");
    expect(held?.permissionText).toContain("the agent wants to install a package");
  });

  it("carries the last assistant message the app holds", () => {
    const held = heldContentForSession(true, undefined, "Deploy finished successfully");
    expect(held?.lastAssistantText).toBe("Deploy finished successfully");
  });

  it("gives nothing for a session this window cannot see, even with content held", () => {
    expect(heldContentForSession(false, { title: "secret work" }, "secret answer")).toBeUndefined();
  });

  it("gives nothing when nothing is held", () => {
    expect(heldContentForSession(true, undefined)).toBeUndefined();
    expect(heldContentForSession(true, undefined, "   ")).toBeUndefined();
  });
});

describe("workspaceHeldContentProvider", () => {
  it("quotes the strip's row, the pending card and the held transcript", () => {
    const provider = workspaceHeldContentProvider({
      rendered: () => true,
      pending: () => ({ title: "Run npm install" }),
      heldAssistantText: () => "Deploy finished successfully",
    });
    expect(provider("agent-1")?.permissionText).toContain("Run npm install");
    expect(provider("agent-1")?.lastAssistantText).toContain("Deploy finished");
  });

  it("asks the inputs on every call, so a row that left the strip is not quoted", () => {
    let rendered = true;
    const provider = workspaceHeldContentProvider({
      rendered: () => rendered,
      pending: () => ({ title: "Run npm install" }),
      heldAssistantText: () => "Deploy finished successfully",
    });
    expect(provider("agent-1")).toBeDefined();
    rendered = false;
    expect(provider("agent-1")).toBeUndefined();
  });
});

describe("sendWithPermission", () => {
  // The answer under test is the plugin's RUST side. The shape has no web
  // permission at all, because WebView2's web value always says "denied" —
  // the live check measured the Rust side answering granted at the same
  // moment.
  it("sends when the Rust side says granted", async () => {
    const plugin = {
      rustPermissionGranted: vi.fn(async () => true),
      sendNotification: vi.fn(async () => undefined),
    };
    await sendWithPermission({ title: "t", body: "b" }, plugin);
    expect(plugin.sendNotification).toHaveBeenCalledWith({ title: "t", body: "b" });
  });

  it("throws on a Rust-side denial, and sends nothing", async () => {
    const plugin = {
      rustPermissionGranted: vi.fn(async () => false),
      sendNotification: vi.fn(async () => undefined),
    };
    await expect(sendWithPermission({ title: "t", body: "b" }, plugin)).rejects.toThrow(
      "the OS permission answer was not granted",
    );
    expect(plugin.sendNotification).not.toHaveBeenCalled();
  });

  it("asks the Rust side again on the next raise: no denial is cached", async () => {
    // The query is one cheap in-process invoke, so a transient `false` is
    // re-asked, never cached into a permanent silence.
    let granted = false;
    const plugin = {
      rustPermissionGranted: vi.fn(async () => granted),
      sendNotification: vi.fn(async () => undefined),
    };
    await expect(sendWithPermission({ title: "t", body: "b" }, plugin)).rejects.toThrow();
    granted = true;
    await sendWithPermission({ title: "t2", body: "b2" }, plugin);
    expect(plugin.rustPermissionGranted).toHaveBeenCalledTimes(2);
    expect(plugin.sendNotification).toHaveBeenCalledTimes(1);
  });
});

describe("fireAttentionToast delivery", () => {
  const hidden = {
    windowState: async () => ({ visible: false, focused: false, minimized: false }),
  };

  it("retries a failed raise once after the delay, then drops it", async () => {
    vi.useFakeTimers();
    forgetAttentionFor(new Set());
    const send = vi.fn(async () => {
      throw new Error("the toast did not land");
    });
    fireAttentionToast("s1", "agent one", attention("finished", 1000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(TOAST_RETRY_DELAY_MS);
    expect(send).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(TOAST_RETRY_DELAY_MS);
    expect(send).toHaveBeenCalledTimes(2);
    vi.useRealTimers();
  });

  it("reaches a raising session through the roster observer, one retry from the timer", async () => {
    vi.useFakeTimers();
    forgetAttentionFor(new Set());
    const send = vi.fn(async () => {
      throw new Error("the toast did not land");
    });
    const watched: {
      listener: ((snapshots: SessionStateSnapshot[]) => void) | null;
    } = { listener: null };
    const deps: Partial<ToastDeps> = { send, ...hidden };
    const controller = createWorkspaceSessionController(
      {
        list: vi.fn(async () => []),
        create: vi.fn(async () => {
          throw new Error("not created here");
        }),
        watch: vi.fn(async (listener) => {
          watched.listener = listener;
          return () => {
            watched.listener = null;
          };
        }),
      },
      (session, raised) => fireAttentionToast(session.id, sessionTitle(session), raised, deps),
    );
    const snapshot = (atMs: number): SessionStateSnapshot[] => [
      {
        id: "agent-1",
        workspaceId: null,
        kind: "acp",
        title: "agent one",
        state: { type: "live", generation: 1 },
        elapsedMs: 0,
        attention: { reason: "finished", atMs },
      },
    ];
    const release = controller.watch();
    // The first roster is the baseline, and the second carries the SAME
    // timestamp: neither is a new raise, so nothing fires yet.
    watched.listener?.(snapshot(1000));
    watched.listener?.(snapshot(1000));
    expect(send).not.toHaveBeenCalled();
    // A newer raise fires once, after the window state answers.
    watched.listener?.(snapshot(2000));
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    // The same raise re-published while the send is failing is filtered by
    // the controller — the retry must come from the timer, not this push.
    watched.listener?.(snapshot(2000));
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(TOAST_RETRY_DELAY_MS);
    expect(send).toHaveBeenCalledTimes(2);
    release();
    vi.useRealTimers();
  });

  it("does not retry a raise that was delivered", async () => {
    vi.useFakeTimers();
    forgetAttentionFor(new Set());
    const send = vi.fn(async () => undefined);
    fireAttentionToast("s2", "agent two", attention("finished", 1000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    fireAttentionToast("s2", "agent two", attention("finished", 1000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });

  it("hands the provider's held content to the sender", async () => {
    vi.useFakeTimers();
    forgetAttentionFor(new Set());
    setAttentionHeldContentProvider((sessionId) =>
      sessionId === "s3" ? { permissionText: "Run npm install" } : undefined,
    );
    const send = vi.fn(async (_content: ToastContent) => undefined);
    fireAttentionToast("s3", "agent three", attention("permission", 1000), {
      send,
      ...hidden,
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    const sent = send.mock.calls[0]?.[0];
    expect(sent?.body).toBe("Run npm install");
    setAttentionHeldContentProvider(null);
    vi.useRealTimers();
  });
});

describe("fireAttentionToast ordering and rejection", () => {
  it("does not send a stale raise after a newer one", async () => {
    vi.useFakeTimers();
    forgetAttentionFor(new Set());
    const { windowState, resolveNext } = deferredWindowState();
    const send = vi.fn(async (_content: ToastContent) => undefined);
    const deps = { send, windowState };
    const hidden = { visible: false, focused: false, minimized: false };
    // The older raise pauses in its window-state read; the newer one (same
    // session) takes the slot and resolves first.
    fireAttentionToast("s9", "agent nine", attention("finished", 1000), deps);
    fireAttentionToast("s9", "agent nine", attention("finished", 2000), deps);
    resolveNext(hidden); // the newer raise's read
    await vi.advanceTimersByTimeAsync(0);
    resolveNext(hidden); // the older raise's read, late
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });

  it("toasts when the window-state read rejects", async () => {
    // The live rule: when the app cannot tell whether the user is looking,
    // it behaves as if they are not.
    forgetAttentionFor(new Set());
    documentClaimsVisibleAndFocused();
    const send = vi.fn(async (_content: ToastContent) => undefined);
    fireAttentionToast("s10", "agent ten", attention("finished", 1000), {
      send,
      windowState: async () => {
        throw new Error("the window could not be asked");
      },
    });
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    expect(send).toHaveBeenCalledTimes(1);
  });
});

describe("fireAttentionToast window gate", () => {
  // The document is mocked into the lying state the live check measured:
  // hidden window, but the page still claims visible and focused. The gate
  // must ask the window, not the page.
  beforeEach(() => {
    vi.useFakeTimers();
    documentClaimsVisibleAndFocused();
    forgetAttentionFor(new Set());
    // No surface reports a selection here, so a seen window is not enough to
    // hold any of these raises back — the per-session clause of the gate.
    reportSelection(null);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("toasts a hidden-but-document-visible window", async () => {
    const send = vi.fn(async (_content: ToastContent) => undefined);
    fireAttentionToast("g1", "agent one", attention("finished", 1000), {
      send,
      windowState: hiddenInTray,
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("stays silent for the session this window shows in a seen window", async () => {
    const send = vi.fn(async (_content: ToastContent) => undefined);
    reportSelection("g2");
    fireAttentionToast("g2", "agent two", attention("finished", 1000), {
      send,
      windowState: onScreenFocused,
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();
    reportSelection(null);
  });

  it("toasts a same-millisecond escalation of an announced raise", async () => {
    forgetAttentionFor(new Set());
    const send = vi.fn(async (_content: ToastContent) => undefined);
    fireAttentionToast("e0", "agent zero", attention("finished", 1000), {
      send,
      windowState: hiddenInTray,
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    // The same millisecond, a different reason: the second produces a toast.
    fireAttentionToast("e0", "agent zero", attention("permission", 1000), {
      send,
      windowState: hiddenInTray,
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(2);
  });

  it("does not let a late downgrade invalidate the pending escalation toast", async () => {
    forgetAttentionFor(new Set());
    // The permission raise is pending in its window-state read...
    let resolvePermissionRead: (state: WindowState) => void = () => undefined;
    const permissionWindowState = (): Promise<WindowState> =>
      new Promise((resolve) => {
        resolvePermissionRead = resolve;
      });
    const send = vi.fn(async (_content: ToastContent) => undefined);
    fireAttentionToast("d1", "agent one", attention("permission", 1000), {
      send,
      windowState: permissionWindowState,
    });
    // ...a delayed roster push arrives with finished at the SAME stamp: a
    // downgrade, not a raise. It must be ignored entirely.
    fireAttentionToast("d1", "agent one", attention("finished", 1000), {
      send,
      windowState: hiddenInTray,
    });
    resolvePermissionRead({ visible: false, focused: false, minimized: false });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    const sent = send.mock.calls[0]?.[0];
    expect(sent?.title).toContain("needs approval");
    vi.useRealTimers();
  });

  it("toasts a minimized window", async () => {
    const send = vi.fn(async (_content: ToastContent) => undefined);
    fireAttentionToast("g3", "agent three", attention("permission", 1000), {
      send,
      windowState: minimized,
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
  });
});

describe("the toast gate is Paseo's rule", () => {
  // The production path: the real roster controller with the observer wiring
  // `sharedSessionController` installs (one `fireAttentionToast` per NEW
  // raise), the real held-content provider, and the real `reportSelection`
  // bridge the Workspace effect drives — the real presence reporter where it
  // matters. Only the OS is doubled: the window-state read and the sender,
  // because a test has no window to hide and no toast service to read.
  let watched: { listener: ((snapshots: SessionStateSnapshot[]) => void) | null };

  function harness(
    send: (content: ToastContent) => Promise<void>,
    windowState: () => Promise<WindowState>,
    list: () => Promise<Session[]> = async () => [],
  ) {
    watched = { listener: null };
    const controller = createWorkspaceSessionController(
      {
        list: vi.fn(list),
        create: vi.fn(async () => {
          throw new Error("not created here");
        }),
        watch: vi.fn(async (listener) => {
          watched.listener = listener;
          return () => {
            watched.listener = null;
          };
        }),
      },
      (session, raised) =>
        fireAttentionToast(session.id, sessionTitle(session), raised, { send, windowState }),
    );
    const release = controller.watch();
    return { controller, release };
  }

  /** Two live sessions in two workspaces; only one is in the strip at a time. */
  function roster(raisedId: string | null, atMs = 2000): SessionStateSnapshot[] {
    return [
      ["workspace-a", "agent-one"],
      ["workspace-b", "agent-two"],
    ].map(([workspaceId, id]) => ({
      id,
      workspaceId,
      kind: "acp" as const,
      title: id,
      state: { type: "live" as const, generation: 1 },
      elapsedMs: 0,
      ...(raisedId === id ? { attention: { reason: "finished" as const, atMs } } : {}),
    }));
  }

  beforeEach(() => {
    vi.useFakeTimers();
    documentClaimsVisibleAndFocused();
    forgetAttentionFor(new Set());
    setAttentionHeldContentProvider(null);
    reportSelection(null);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("announces another session's raise while this window stays focused", async () => {
    const send = vi.fn(async (_content: ToastContent) => undefined);
    const { release } = harness(send, onScreenFocused);
    reportSelection("agent-one");
    watched.listener?.(roster(null));
    watched.listener?.(roster("agent-two"));
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send.mock.calls[0]?.[0].title).toBe("agent-two — finished");
    release();
  });

  it("stays silent for the raise of the session this window shows", async () => {
    const send = vi.fn(async (_content: ToastContent) => undefined);
    const { release } = harness(send, onScreenFocused);
    reportSelection("agent-two");
    watched.listener?.(roster(null));
    watched.listener?.(roster("agent-two"));
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();
    release();
  });

  it("announces every raise while the window is hidden, the shown session included", async () => {
    const send = vi.fn(async (_content: ToastContent) => undefined);
    const { release } = harness(send, hiddenInTray);
    reportSelection("agent-one");
    watched.listener?.(roster(null));
    watched.listener?.(roster("agent-one"));
    watched.listener?.(roster("agent-two", 3000));
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(2);
    release();
  });

  it("announces one raise once, however often the roster re-publishes it", async () => {
    // The away case, twice over: the controller offers the standing raise on
    // every application (Paseo's notifier is fed by the events themselves), and
    // the notifier's own record — the one that was never written for a raise the
    // gate suppressed — is what keeps this to one toast.
    const send = vi.fn(async (_content: ToastContent) => undefined);
    const { release } = harness(send, hiddenInTray);
    reportSelection("agent-one");
    watched.listener?.(roster(null));
    watched.listener?.(roster("agent-two"));
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    watched.listener?.(roster("agent-two"));
    watched.listener?.(roster("agent-two"));
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    release();
  });

  it("announces a raise the gate suppressed once the user has looked away", async () => {
    // The review's P1: Paseo records ONLY after its gate, so a raise held back
    // for a user who was looking at it stays due — and the next publication of
    // the same event announces it once they have switched away. The controller
    // offers that publication because it no longer filters the raise itself.
    const send = vi.fn(async (_content: ToastContent) => undefined);
    const { controller, release } = harness(send, onScreenFocused);
    reportSelection("agent-one");
    watched.listener?.(roster(null));
    watched.listener?.(roster("agent-one"));
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();

    controller.select("agent-two");
    reportSelection(controller.getState().selectedSessionId);
    watched.listener?.(roster("agent-one"));
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send.mock.calls[0]?.[0].title).toBe("agent-one — finished");
    release();
  });

  it("announces one of two publications in flight for the same raise", async () => {
    // The window answer is an await, so two publications of one raise can be in
    // flight together. Paseo's record is written after the gate; the two
    // continuations still cannot both toast, because the claim test and the
    // write share no await. Deferred reads make that interleaving real.
    const { windowState, resolveNext } = deferredWindowState();
    const send = vi.fn(async (_content: ToastContent) => undefined);
    const { release } = harness(send, windowState);
    reportSelection("agent-one");
    watched.listener?.(roster(null));
    watched.listener?.(roster("agent-two"));
    watched.listener?.(roster("agent-two"));
    resolveNext({ visible: false, focused: false, minimized: false });
    await vi.advanceTimersByTimeAsync(0);
    resolveNext({ visible: false, focused: false, minimized: false });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    release();
  });

  it("holds a raise back for the session the record names, over one record", async () => {
    // One write, two readers: the surface's selection reaches the daemon through
    // the real reporter AND decides the gate, from the same record. The TIMING of
    // a real switch — the write landing with the click that decided it — is
    // pinned in WorkspaceAttentionToast.test.tsx, where no test hand-calls the
    // bridge; here the point is only that one record answers both questions.
    const send = vi.fn(async (_content: ToastContent) => undefined);
    const presence = vi.fn(async (_args: unknown) => undefined);
    const reporter = startPresenceReporting({
      invoke: presence as unknown as PresenceDeps["invoke"],
      window: {
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
      },
      document: {
        visibilityState: "visible",
        hasFocus: () => true,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
      },
      windowState: onScreenFocused,
    });
    const { release } = harness(send, onScreenFocused);
    reportSelection("agent-one");
    watched.listener?.(roster(null));

    // What the surface reports when its strip now shows agent-two, and the raise
    // is published after that.
    reportSelection("agent-two");
    await vi.advanceTimersByTimeAsync(0);
    expect(presence).toHaveBeenLastCalledWith("session_presence", {
      focusedSessionId: "agent-two",
      appVisible: true,
    });

    watched.listener?.(roster("agent-two"));
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();
    reporter.dispose();
    release();
  });

  it("a surface without a strip shows no session, so every raise announces", async () => {
    // Settings or Design on screen: the Workspace unmounted and withdrew its
    // selection, so nothing may be held back as "already looked at".
    const send = vi.fn(async (_content: ToastContent) => undefined);
    const { release } = harness(send, onScreenFocused);
    reportSelection("agent-one");
    watched.listener?.(roster(null));
    reportSelection(null);
    watched.listener?.(roster("agent-one"));
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    release();
  });

  it("quotes the words of a row this window renders, and nothing for the rest", async () => {
    const send = vi.fn(async (_content: ToastContent) => undefined);
    const { release } = harness(send, hiddenInTray);
    reportSelection("agent-one");
    setAttentionHeldContentProvider(
      workspaceHeldContentProvider({
        rendered: (sessionId) => sessionId === "agent-one",
        pending: () => undefined,
        heldAssistantText: (sessionId) =>
          sessionId === "agent-one" ? "Deploy finished successfully" : undefined,
      }),
    );
    watched.listener?.(roster(null));
    watched.listener?.(roster("agent-one"));
    watched.listener?.(roster("agent-two", 3000));
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(2);
    // B2's live check: the tray toast for the session the user was reading
    // quotes its answer; another session's toast says the reason and stops.
    expect(send.mock.calls[0]?.[0].body).toBe("Deploy finished successfully");
    expect(send.mock.calls[1]?.[0].body).toBe("finished");
    release();
  });

  it("a refresh that drops a row from the strip does not re-announce its raise", async () => {
    // `stripSessions` drops an ended row the daemon still lists. The dedupe
    // is keyed to the daemon's roster, not to the strip's view, so the
    // standing raise stays spent and the next push is silence: pruning
    // against the stripped set made one event toast twice.
    const send = vi.fn(async (_content: ToastContent) => undefined);
    let ended = false;
    const { controller, release } = harness(send, onScreenFocused, async () => [
      {
        id: "agent-two",
        workspaceId: "workspace-b",
        kind: "acp" as const,
        title: "agent-two",
        state: ended
          ? {
              type: "ended" as const,
              generation: 1,
              code: 0,
              integrity: { kind: "complete" as const },
            }
          : ({ type: "live", generation: 1 } as const),
        elapsedMs: 0,
      },
    ]);
    reportSelection("agent-one");
    watched.listener?.(roster(null));
    watched.listener?.(roster("agent-two"));
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);

    ended = true;
    await controller.refresh();
    expect(controller.getState().sessions.map((session) => session.id)).not.toContain("agent-two");

    // The daemon lists the row again with the SAME raise standing: silence.
    ended = false;
    watched.listener?.(roster("agent-two"));
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    release();
  });
});
