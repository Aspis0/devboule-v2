// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Attention } from "../../types/ipc";
import type { ToastContent, WindowState } from "./attentionNotice";
import {
  TOAST_RETRY_DELAY_MS,
  fireAttentionToast,
  forgetAttentionFor,
  setAttentionHeldContentProvider,
} from "./attentionNotice";
import { setShowMessagePreviews, setShowNotifications } from "../../lib/notificationPrefs";
import { reportSelection } from "./presence";

function attention(reason: Attention["reason"], atMs: number): Attention {
  return { reason, atMs };
}

const hidden = {
  windowState: async (): Promise<WindowState> => ({
    visible: false,
    focused: false,
    minimized: false,
  }),
};

describe("fireAttentionToast honours the notification preferences", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    forgetAttentionFor(new Set());
    setAttentionHeldContentProvider((sessionId) =>
      sessionId === "s1" ? { lastAssistantText: "Deploy finished successfully" } : undefined,
    );
    reportSelection(null);
    setShowNotifications(true);
    setShowMessagePreviews(true);
  });

  afterEach(() => {
    setAttentionHeldContentProvider(null);
    setShowNotifications(true);
    setShowMessagePreviews(true);
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("stays silent while Show notifications is off", async () => {
    setShowNotifications(false);
    const send = vi.fn(async (_content: ToastContent) => undefined);
    fireAttentionToast("s1", "agent one", attention("finished", 1000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();
  });

  it("consumes raises that arrive while off: switching back on fires nothing old", async () => {
    // The owner's F8: a raise that arrives while the master switch is off
    // is consumed, not kept due — turning the switch back on announces new
    // raises only, never a burst of old toasts.
    setShowNotifications(false);
    const send = vi.fn(async (_content: ToastContent) => undefined);
    fireAttentionToast("s1", "agent one", attention("finished", 1000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    fireAttentionToast("s1", "agent one", attention("finished", 2000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();

    setShowNotifications(true);
    fireAttentionToast("s1", "agent one", attention("finished", 2000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();

    // A genuinely new raise after the switch still announces.
    fireAttentionToast("s1", "agent one", attention("finished", 3000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("names the session and the reason but carries no preview text while previews are off", async () => {
    setShowMessagePreviews(false);
    const send = vi.fn(async (_content: ToastContent) => undefined);
    fireAttentionToast("s1", "agent one", attention("finished", 1000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    const sent = send.mock.calls[0]?.[0];
    expect(sent?.title).toContain("agent one");
    expect(sent?.body).toBe("finished");
    expect(sent?.body).not.toContain("Deploy finished");
  });

  it("rebuilds the content at retry time: no frozen preview text after the switch", async () => {
    // F1: the first send fails while previews are on; the user switches
    // previews off inside the 1.5 s window. The retry must carry no message
    // text — never a frozen object built before the switch moved.
    const send = vi.fn(async (_content: ToastContent) => {
      throw new Error("the toast did not land");
    });
    fireAttentionToast("s1", "agent one", attention("finished", 1000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send.mock.calls[0]?.[0].body).toContain("Deploy finished");

    setShowMessagePreviews(false);
    await vi.advanceTimersByTimeAsync(TOAST_RETRY_DELAY_MS);
    expect(send).toHaveBeenCalledTimes(2);
    const retried = send.mock.calls[1]?.[0];
    expect(retried?.title).toContain("agent one");
    expect(retried?.body).toBe("finished");
    expect(retried?.body).not.toContain("Deploy finished");
  });

  it("drops the retry when the master switch is off at retry time", async () => {
    const send = vi.fn(async (_content: ToastContent) => {
      throw new Error("the toast did not land");
    });
    fireAttentionToast("s4", "agent four", attention("finished", 1000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);

    setShowNotifications(false);
    await vi.advanceTimersByTimeAsync(TOAST_RETRY_DELAY_MS);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("re-reads the master switch after the async hops: a mid-flight off sends nothing and records nothing", async () => {
    // F2: the raise arrives while the switch is on, but the user switches
    // it off while the window-state read is in flight. The toast must not
    // land — and must not be recorded as fired, so the next publication of
    // the same raise announces once the switch is back on.
    const resolvers: Array<(state: WindowState) => void> = [];
    const windowState = (): Promise<WindowState> =>
      new Promise<WindowState>((resolve) => {
        resolvers.push(resolve);
      });
    const send = vi.fn(async (_content: ToastContent) => undefined);
    fireAttentionToast("s5", "agent five", attention("finished", 1000), { send, windowState });
    setShowNotifications(false);
    resolvers.shift()?.({ visible: false, focused: false, minimized: false });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();

    setShowNotifications(true);
    fireAttentionToast("s5", "agent five", attention("finished", 1000), {
      send,
      ...hidden,
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("reads the preferences at fire time, not from an earlier answer", async () => {
    const send = vi.fn(async (_content: ToastContent) => undefined);
    fireAttentionToast("s1", "agent one", attention("finished", 1000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);

    // The human flips both switches after the first toast landed; the next
    // raise obeys the new answers with no remount in between.
    setShowMessagePreviews(false);
    setShowNotifications(false);
    fireAttentionToast("s1", "agent one", attention("finished", 2000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(1);

    setShowNotifications(true);
    fireAttentionToast("s1", "agent one", attention("finished", 3000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).toHaveBeenCalledTimes(2);
    expect(send.mock.calls[1]?.[0].body).toBe("finished");
  });
});
