// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Attention, AttentionReason, SessionStateSnapshot } from "../../types/ipc";
import type { ToastContent, ToastDeps, WindowState } from "./attentionNotice";
import {
  setAttentionHeldContentProvider,
  flushParkedAttentionRaises,
  PREVIEW_LIMIT,
  TOAST_RETRY_DELAY_MS,
  attentionRaised,
  fireAttentionToast,
  forgetAttentionFor,
  heldContentForSession,
  previewFrom,
  sendWithPermission,
  stripMarkdown,
  toastContent,
  toastGate,
  workspaceHeldContentProvider,
} from "./attentionNotice";
import { startPresenceReporting, type PresenceReporter } from "./presence";
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
  it("stays silent while the window is focused, visible and not minimized", () => {
    expect(toastGate({ visible: true, focused: true, minimized: false })).toBe(false);
  });

  it("fires when the window is hidden, minimized, or unfocused", () => {
    expect(toastGate({ visible: false, focused: false, minimized: false })).toBe(true); // in the tray
    expect(toastGate({ visible: true, focused: true, minimized: true })).toBe(true); // minimized
    expect(toastGate({ visible: true, focused: false, minimized: false })).toBe(true); // behind
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
    expect(provider.heldContent("agent-1")?.permissionText).toContain("Run npm install");
    expect(provider.heldContent("agent-1")?.lastAssistantText).toContain("Deploy finished");
  });

  it("asks the inputs on every call, so a hidden row is not quoted", () => {
    let rendered = true;
    const provider = workspaceHeldContentProvider({
      rendered: () => rendered,
      pending: () => ({ title: "Run npm install" }),
      heldAssistantText: () => "Deploy finished successfully",
    });
    expect(provider.heldContent("agent-1")).toBeDefined();
    rendered = false;
    expect(provider.heldContent("agent-1")).toBeUndefined();
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
    setAttentionHeldContentProvider({
      heldContent: (sessionId) =>
        sessionId === "s3" ? { permissionText: "Run npm install" } : undefined,
      rendered: () => true,
      pending: () => undefined,
    });
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
  // Deferred window-state reads, resolved per fire: the only way to test
  // the order two raises land their toasts in.
  const deferredWindowState = (): {
    windowState: () => Promise<WindowState>;
    resolveNext: (state: WindowState) => void;
  } => {
    const resolvers: Array<(state: WindowState) => void> = [];
    return {
      windowState: () =>
        new Promise<WindowState>((resolve) => {
          resolvers.push(resolve);
        }),
      resolveNext: (state) => resolvers.shift()?.(state),
    };
  };

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

  it("stays silent for a window that is visible and focused", async () => {
    const send = vi.fn(async (_content: ToastContent) => undefined);
    fireAttentionToast("g2", "agent two", attention("finished", 1000), {
      send,
      windowState: onScreenFocused,
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();
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

describe("fireAttentionToast seen gate needs the row rendered", () => {
  // The strip is scoped to the selected workspace, so a focused window can
  // still not render the raising session's row (another workspace's tab,
  // search hidden). "Seen" is window seen AND row rendered: a raise whose
  // row nobody saw is PARKED, and what announces it is the presence
  // reporter's seen→unseen transition flushing the parked set — never a
  // hand-called second fire.
  let watchedFocus: { handler: ((event: { payload: boolean }) => void) | null };
  let windowAnswer: WindowState;
  let reporter: PresenceReporter | null = null;

  // The production wiring, with this file's usual seams: the reporter owns
  // the unseen transition; the flush callback is wired exactly like
  // Workspace wires it, plus this file's injected send/window state.
  const startReporter = (send: (content: ToastContent) => Promise<void>): void => {
    windowAnswer = { visible: true, focused: true, minimized: false };
    watchedFocus = { handler: null };
    reporter = startPresenceReporting({
      invoke: vi.fn(async () => undefined),
      windowState: async () => windowAnswer,
      onWindowFocusChange: async (handler) => {
        watchedFocus.handler = handler;
        return () => undefined;
      },
      onWindowBecameUnseen: () =>
        flushParkedAttentionRaises({ send, windowState: async () => windowAnswer }),
    });
  };

  beforeEach(() => {
    vi.useFakeTimers();
    forgetAttentionFor(new Set());
  });

  afterEach(() => {
    reporter?.dispose();
    reporter = null;
    setAttentionHeldContentProvider(null);
    vi.useRealTimers();
  });

  it("an off-strip raise seen while focused announces once when presence reports unseen", async () => {
    const send = vi.fn(async (_content: ToastContent) => undefined);
    startReporter(send);
    await vi.advanceTimersByTimeAsync(0);
    setAttentionHeldContentProvider(
      workspaceHeldContentProvider({
        rendered: () => false,
        pending: () => ({ title: "Run npm install" }),
        heldAssistantText: () => undefined,
      }),
    );
    fireAttentionToast("v1", "agent one", attention("permission", 1000), {
      send,
      windowState: async () => windowAnswer,
    });
    await vi.advanceTimersByTimeAsync(0);
    // Seen window, unrendered row: parked, not announced, not consumed.
    expect(send).not.toHaveBeenCalled();

    // The user hides the window; the presence reporter applies the unseen
    // answer and the flush announces the parked raise exactly once.
    windowAnswer = { visible: false, focused: false, minimized: false };
    watchedFocus.handler?.({ payload: false });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);

    // A repeated unseen answer (the poll, another blur) does not re-announce.
    watchedFocus.handler?.({ payload: false });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("a parked raise whose row the user reached before hiding does not announce", async () => {
    let rendered = false;
    const send = vi.fn(async (_content: ToastContent) => undefined);
    startReporter(send);
    await vi.advanceTimersByTimeAsync(0);
    setAttentionHeldContentProvider(
      workspaceHeldContentProvider({
        rendered: () => rendered,
        pending: () => ({ title: "Run npm install" }),
        heldAssistantText: () => undefined,
      }),
    );
    fireAttentionToast("v2", "agent two", attention("permission", 1000), {
      send,
      windowState: async () => windowAnswer,
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();

    // The user switches to that workspace (the row renders), then hides.
    rendered = true;
    windowAnswer = { visible: false, focused: false, minimized: false };
    watchedFocus.handler?.({ payload: false });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();
  });

  it("a parked permission raise answered before hiding does not announce", async () => {
    let answered = false;
    const send = vi.fn(async (_content: ToastContent) => undefined);
    startReporter(send);
    await vi.advanceTimersByTimeAsync(0);
    setAttentionHeldContentProvider(
      workspaceHeldContentProvider({
        rendered: () => false,
        pending: () => (answered ? undefined : { title: "Run npm install" }),
        heldAssistantText: () => undefined,
      }),
    );
    fireAttentionToast("v3", "agent three", attention("permission", 1000), {
      send,
      windowState: async () => windowAnswer,
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();

    // The card is answered elsewhere while the raise sits parked, then the
    // window hides: nothing to announce.
    answered = true;
    windowAnswer = { visible: false, focused: false, minimized: false };
    watchedFocus.handler?.({ payload: false });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();
  });

  it("an on-strip raise seen while focused stays consumed when the window hides", async () => {
    const send = vi.fn(async (_content: ToastContent) => undefined);
    startReporter(send);
    await vi.advanceTimersByTimeAsync(0);
    setAttentionHeldContentProvider(
      workspaceHeldContentProvider({
        rendered: () => true,
        pending: () => undefined,
        heldAssistantText: () => undefined,
      }),
    );
    fireAttentionToast("v4", "agent four", attention("permission", 1000), {
      send,
      windowState: async () => windowAnswer,
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();

    // As on main: the raise was genuinely seen and stays consumed — the
    // unseen transition flushes nothing.
    windowAnswer = { visible: false, focused: false, minimized: false };
    watchedFocus.handler?.({ payload: false });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();
  });
});
