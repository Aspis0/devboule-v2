import { describe, expect, it, vi } from "vitest";
import type { Attention, SessionStateSnapshot } from "../../types/ipc";
import type { ToastContent, ToastDeps } from "./attentionNotice";
import {
  setAttentionHeldContentProvider,
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
import { createWorkspaceSessionController, sessionTitle } from "./workspaceSessions";

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
});

describe("toastGate", () => {
  it("stays silent while the window is focused and visible", () => {
    expect(toastGate(true, true)).toBe(false);
  });

  it("fires when the window is hidden, minimized, or unfocused", () => {
    expect(toastGate(false, false)).toBe(true); // in the tray
    expect(toastGate(false, true)).toBe(true); // minimized
    expect(toastGate(true, false)).toBe(true); // visible but behind
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

  it("asks the inputs on every call, so a hidden row is not quoted", () => {
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
  it("sends when permission is already granted without asking", async () => {
    const plugin = {
      isPermissionGranted: vi.fn(async () => true),
      requestPermission: vi.fn(async () => "granted" as NotificationPermission),
      sendNotification: vi.fn(async () => undefined),
    };
    await sendWithPermission({ title: "t", body: "b" }, plugin);
    expect(plugin.requestPermission).not.toHaveBeenCalled();
    expect(plugin.sendNotification).toHaveBeenCalledWith({ title: "t", body: "b" });
  });

  it("asks exactly once when permission was never granted, then sends", async () => {
    let granted = false;
    const plugin = {
      isPermissionGranted: vi.fn(async () => granted),
      requestPermission: vi.fn(async () => {
        granted = true;
        return "granted" as NotificationPermission;
      }),
      sendNotification: vi.fn(async () => undefined),
    };
    await sendWithPermission({ title: "t", body: "b" }, plugin);
    await sendWithPermission({ title: "t2", body: "b2" }, plugin);
    expect(plugin.requestPermission).toHaveBeenCalledTimes(1);
    expect(plugin.sendNotification).toHaveBeenCalledTimes(2);
  });

  it("sends nothing after a denial, and never asks a second time", async () => {
    const plugin = {
      isPermissionGranted: vi.fn(async () => false),
      requestPermission: vi.fn(async () => "denied" as NotificationPermission),
      sendNotification: vi.fn(async () => undefined),
    };
    await sendWithPermission({ title: "t", body: "b" }, plugin);
    await sendWithPermission({ title: "t2", body: "b2" }, plugin);
    expect(plugin.requestPermission).toHaveBeenCalledTimes(1);
    expect(plugin.sendNotification).not.toHaveBeenCalled();
  });
});

describe("fireAttentionToast delivery", () => {
  const hidden = { visible: () => false, focused: () => false };

  it("retries a failed raise once after the delay, then drops it", async () => {
    vi.useFakeTimers();
    forgetAttentionFor(new Set());
    const send = vi.fn(async () => {
      throw new Error("the toast did not land");
    });
    fireAttentionToast("s1", "agent one", attention("finished", 1000), { send, ...hidden });
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
    // A newer raise fires once.
    watched.listener?.(snapshot(2000));
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
    forgetAttentionFor(new Set());
    const send = vi.fn(async () => undefined);
    fireAttentionToast("s2", "agent two", attention("finished", 1000), { send, ...hidden });
    await Promise.resolve();
    await Promise.resolve();
    fireAttentionToast("s2", "agent two", attention("finished", 1000), { send, ...hidden });
    await Promise.resolve();
    await Promise.resolve();
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("hands the provider's held content to the sender", async () => {
    forgetAttentionFor(new Set());
    setAttentionHeldContentProvider((sessionId) =>
      sessionId === "s3" ? { permissionText: "Run npm install" } : undefined,
    );
    const send = vi.fn(async (_content: ToastContent) => undefined);
    fireAttentionToast("s3", "agent three", attention("permission", 1000), {
      send,
      ...hidden,
    });
    await Promise.resolve();
    expect(send).toHaveBeenCalledTimes(1);
    const sent = send.mock.calls[0]?.[0];
    expect(sent?.body).toBe("Run npm install");
    setAttentionHeldContentProvider(null);
  });
});
