// The session-channel mock the surface tests share: the daemon pipe's event
// delivery — `createSessionChannel` for the roster, `sessionAttach` for the
// attached session — behind the two handles a test drives (`emit` and
// `active`), plus the deferral seam that holds an attach open. The surface's
// own suite and the subagent menu's render through it, so the two cannot
// drift apart the way two copies of one mock do.
import { vi } from "vitest";
import type { SessionEvent } from "../../types/ipc";

export const channelHarness = {
  emit: null as ((event: SessionEvent) => void) | null,
  active: null as ((event: SessionEvent) => void) | null,
  activeSubscriptionId: null as number | null,
  nextSubscriptionId: 41,
  deferNextAttach: false,
  releaseNextAttach: null as (() => void) | null,
  handlers: new WeakMap<object, (event: SessionEvent) => void>(),
};

// Consuming test files install this with their own hoisted vi.mock — a
// vi.mock here would only apply to modules resolved after this one.
export const tauriMock = {
  // `workspaceSessions.ts` reads `sessionsList` at module scope for its
  // default source; the roster itself arrives as a prop.
  sessionsList: vi.fn(async () => []),
  createSessionChannel: vi.fn((onEvent: (event: SessionEvent) => void) => {
    const channel = {};
    channelHarness.handlers.set(channel, onEvent);
    channelHarness.emit = onEvent;
    return channel;
  }),
  sessionAttach: vi.fn(async (...args: unknown[]) => {
    if (channelHarness.deferNextAttach) {
      channelHarness.deferNextAttach = false;
      await new Promise<void>((resolve) => {
        channelHarness.releaseNextAttach = resolve;
      });
      channelHarness.releaseNextAttach = null;
    }
    await Promise.resolve();
    const channel = args[2];
    const subscriptionId = channelHarness.nextSubscriptionId++;
    channelHarness.activeSubscriptionId = subscriptionId;
    channelHarness.active =
      typeof channel === "object" && channel !== null
        ? (channelHarness.handlers.get(channel) ?? null)
        : null;
    return subscriptionId;
  }),
  sessionDetach: vi.fn(async (subscriptionId: number) => {
    if (channelHarness.activeSubscriptionId !== subscriptionId) return;
    channelHarness.activeSubscriptionId = null;
    channelHarness.active = null;
  }),
  sessionSend: vi.fn(async () => undefined),
  sessionInterrupt: vi.fn(async () => undefined),
  sessionDeposit: vi.fn(async () => ({
    sessionId: "agent-1",
    digest: "a".repeat(64),
    storedBytes: 1,
  })),
  sessionQueueAdd: vi.fn(async () => undefined),
  sessionQueueEdit: vi.fn(async () => undefined),
  sessionQueueRemove: vi.fn(async () => undefined),
  sessionQueueMove: vi.fn(async () => undefined),
  sessionQueueSendNow: vi.fn(async () => undefined),
  sessionSetModel: vi.fn(async () => undefined),
  sessionSetMode: vi.fn(async () => undefined),
  sessionSetFeature: vi.fn(async () => undefined),
  isCommandError: (error: unknown): boolean =>
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    "message" in error &&
    typeof (error as { code: unknown }).code === "string" &&
    typeof (error as { message: unknown }).message === "string",
};
