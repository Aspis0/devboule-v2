// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Attention } from "../../types/ipc";
import type { ToastContent, WindowState } from "./attentionNotice";
import {
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

  it("leaves the silenced raise due: turning back on announces it", async () => {
    // A raise suppressed by the preference is not consumed — like a raise
    // the window gate held back, the next publication still announces it.
    setShowNotifications(false);
    const send = vi.fn(async (_content: ToastContent) => undefined);
    fireAttentionToast("s1", "agent one", attention("finished", 1000), { send, ...hidden });
    await vi.advanceTimersByTimeAsync(0);
    expect(send).not.toHaveBeenCalled();

    setShowNotifications(true);
    fireAttentionToast("s1", "agent one", attention("finished", 1000), { send, ...hidden });
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
