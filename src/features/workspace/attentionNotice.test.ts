// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Attention, AttentionReason, Session, SessionStateSnapshot } from "../../types/ipc";
import type { ToastContent, ToastDeps, WindowState } from "./attentionNotice";
import {
  setAttentionHeldContentProvider,
  flushParkedAttentionRaises,
  noteRosterAttention,
  noteWindowUnseen,
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
    noteRosterAttention([]);
    // The window's last applied truth persists across tests, like the maps.
    noteWindowUnseen(false);
  });

  afterEach(() => {
    reporter?.dispose();
    reporter = null;
    setAttentionHeldContentProvider(null);
    vi.useRealTimers();
  });

  it("an off-strip permission raise seen while focused announces once when presence reports unseen", async () => {
    // Production truth for an off-strip session: the app never attached to
    // it, so no permission card exists locally and the rendered predicate
    // says no. The roster — not the queue — is what keeps the raise due.
    const send = vi.fn(async (_content: ToastContent) => undefined);
    startReporter(send);
    await vi.advanceTimersByTimeAsync(0);
    setAttentionHeldContentProvider(
      workspaceHeldContentProvider({
        rendered: () => false,
        heldAssistantText: () => undefined,
        pending: () => undefined,
      }),
    );
    const raise = attention("permission", 1000);
    noteRosterAttention([{ id: "v1", attention: raise }]);
    fireAttentionToast("v1", "agent one", raise, {
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
        heldAssistantText: () => undefined,
        pending: () => undefined,
      }),
    );
    const raise = attention("permission", 1000);
    noteRosterAttention([{ id: "v2", attention: raise }]);
    fireAttentionToast("v2", "agent two", raise, {
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

  it("a parked raise whose attention the daemon cleared before the hide does not announce", async () => {
    const send = vi.fn(async (_content: ToastContent) => undefined);
    startReporter(send);
    await vi.advanceTimersByTimeAsync(0);
    setAttentionHeldContentProvider(
      workspaceHeldContentProvider({
        rendered: () => false,
        heldAssistantText: () => undefined,
        pending: () => undefined,
      }),
    );
    const raise = attention("permission", 1000);
    noteRosterAttention([{ id: "v3", attention: raise }]);
    fireAttentionToast("v3", "agent three", raise, {
      send,
      windowState: async () => windowAnswer,
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();

    // The daemon withdraws the raise (the next roster carries no attention
    // for the session), then the window hides: nothing is due any more.
    noteRosterAttention([{ id: "v3", attention: undefined }]);
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
        heldAssistantText: () => undefined,
        pending: () => undefined,
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

  it("a parked raise is never announced once a newer raise for the session has landed", async () => {
    // No provider registered — the surfaces-without-a-strip case, which
    // must park (an unknown rendered answer never claims seen) and must
    // still announce at flush. The newer raise toasts immediately; whether
    // through the dedupe (lastFired holds the newer raise) or the roster
    // check, the stale park must stay silent afterwards.
    const send = vi.fn(async (_content: ToastContent) => undefined);
    const first = attention("finished", 1000);
    noteRosterAttention([{ id: "w1", attention: first }]);
    fireAttentionToast("w1", "agent one", first, { send, windowState: onScreenFocused });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();

    // The window hides and a newer raise lands for the same session: the
    // newer one toasts at once, and the park must not still hold the stale
    // raise behind it.
    const newer = attention("finished", 2000);
    fireAttentionToast("w1", "agent one", newer, { send, windowState: hiddenInTray });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);

    flushParkedAttentionRaises({ send, windowState: hiddenInTray });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("a raise parked by a stale seen read announces at once when the flip already fired", async () => {
    // The raise's window-state read pauses in flight; the window hides and
    // the reporter spends the only flip. When the stale read resolves
    // "seen", parking would wait for a flip that never comes again — the
    // newer window truth says announce now.
    const send = vi.fn(async (_content: ToastContent) => undefined);
    startReporter(send);
    await vi.advanceTimersByTimeAsync(0);
    setAttentionHeldContentProvider(
      workspaceHeldContentProvider({
        rendered: () => false,
        heldAssistantText: () => undefined,
        pending: () => undefined,
      }),
    );
    const { windowState, resolveNext } = deferredWindowState();
    fireAttentionToast("v5", "agent five", attention("finished", 1000), { send, windowState });
    windowAnswer = { visible: false, focused: false, minimized: false };
    watchedFocus.handler?.({ payload: false });
    await vi.advanceTimersByTimeAsync(0);
    // The flip fired on an empty park: nothing announced yet.
    expect(send).not.toHaveBeenCalled();

    resolveNext({ visible: true, focused: true, minimized: false });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);

    // Announced, not parked: another flush adds nothing.
    flushParkedAttentionRaises({ send, windowState: async () => windowAnswer });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("a parked raise in a surface without a strip announces at flush", async () => {
    // No provider registered: an unknown rendered answer must count as NOT
    // seen — the raise parks here and the flush announces it. If unknown
    // meant "seen", this raise would never reach its user.
    const send = vi.fn(async (_content: ToastContent) => undefined);
    const raise = attention("permission", 1000);
    noteRosterAttention([{ id: "np1", attention: raise }]);
    fireAttentionToast("np1", "agent one", raise, { send, windowState: onScreenFocused });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();
    flushParkedAttentionRaises({ send, windowState: hiddenInTray });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
  });
});

describe("the controller's roster notes feed the parked-raise oracle", () => {
  // The oracle has two production writers and this is their only direct
  // test: the push (whose snapshots carry the attention field) is
  // authoritative, while the list refresh's `Session` rows carry no
  // attention field at all — absent is not withdrawn, so the refresh may
  // only prune.
  let watched: { listener: ((snapshots: SessionStateSnapshot[]) => void) | null };

  function makeController(send: (content: ToastContent) => Promise<void>) {
    watched = { listener: null };
    const listed: Session[] = [];
    const controller = createWorkspaceSessionController(
      {
        list: vi.fn(async () => listed),
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
        fireAttentionToast(session.id, sessionTitle(session), raised, {
          send,
          windowState: onScreenFocused,
        }),
    );
    return { controller, listed };
  }

  const push = (atMs: number, withAttention = true): SessionStateSnapshot[] => [
    {
      id: "agent-1",
      workspaceId: null,
      kind: "acp",
      title: "agent one",
      state: { type: "live", generation: 1 },
      elapsedMs: 0,
      ...(withAttention ? { attention: { reason: "finished" as const, atMs } } : {}),
    },
  ];

  const listRow = (): Session => ({
    id: "agent-1",
    workspaceId: null,
    kind: "acp",
    title: "agent one",
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
  });

  beforeEach(() => {
    vi.useFakeTimers();
    forgetAttentionFor(new Set());
    noteRosterAttention([]);
    noteWindowUnseen(false);
    setAttentionHeldContentProvider(null);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("a routine list refresh does not withdraw a parked raise", async () => {
    const send = vi.fn(async (_content: ToastContent) => undefined);
    const { controller, listed } = makeController(send);
    const release = controller.watch();
    // First roster is the baseline; the second is a real raise.
    watched.listener?.(push(1000));
    watched.listener?.(push(2000));
    await vi.advanceTimersByTimeAsync(0);
    // Seen window and no strip here: parked, unannounced.
    expect(send).not.toHaveBeenCalled();

    // The refresh resolves with the reply's real shape: no attention field.
    listed.push(listRow());
    await controller.refresh();

    flushParkedAttentionRaises({ send, windowState: hiddenInTray });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    release();
  });

  it("a push that withdraws the attention takes the parked raise with it", async () => {
    const send = vi.fn(async (_content: ToastContent) => undefined);
    const { controller } = makeController(send);
    const release = controller.watch();
    watched.listener?.(push(1000));
    watched.listener?.(push(2000));
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();

    // The daemon withdraws: a push that carries no attention for the session.
    watched.listener?.(push(3000, false));
    await vi.advanceTimersByTimeAsync(0);

    flushParkedAttentionRaises({ send, windowState: hiddenInTray });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();
    release();
  });
});
