// @vitest-environment happy-dom
// While a plan card waits, the timeline must not show a second copy of the
// plan; when the card resolves, the row returns to its original place. The
// tests drive the production derivation, not a re-implementation of it.
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  PermissionRequest,
  PermissionResolved,
  SessionEvent,
  SessionState,
} from "../../types/ipc";
import type { ReactNode } from "react";
import { pendingPlanId, type RenderedPermissionCard } from "./pendingPlanId";

const channelHarness = vi.hoisted(() => ({
  emit: null as ((event: SessionEvent) => void) | null,
  active: null as ((event: SessionEvent) => void) | null,
  activeSubscriptionId: null as number | null,
  nextSubscriptionId: 41,
  handlers: new WeakMap<object, (event: SessionEvent) => void>(),
}));

vi.mock("../../lib/tauri", () => ({
  sessionsList: vi.fn(async () => []),
  createSessionChannel: vi.fn((onEvent: (event: SessionEvent) => void) => {
    const channel = {};
    channelHarness.handlers.set(channel, onEvent);
    channelHarness.emit = onEvent;
    return channel;
  }),
  sessionAttach: vi.fn(async (...args: unknown[]) => {
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
}));

import { AgentChatSurface } from "./AgentChatSurface";

const SESSION = "plan-dup";

const LIVE: SessionState = { type: "live", generation: 1 };

const PLAN_OPTIONS = [
  { optionId: "deny", name: "Reject", kind: "reject_once" },
  { optionId: "implement", name: "Implement", kind: "allow_once" },
];

const COMMAND_OPTIONS = [
  { optionId: "allow", name: "Allow once", kind: "allow_once" },
  { optionId: "deny", name: "Deny", kind: "reject_once" },
];

function planRequest(toolCallId: string): PermissionRequest {
  return {
    type: "permission_request",
    toolCallId,
    title: "Plan",
    options: PLAN_OPTIONS,
    kind: "plan",
    plan: "## Steps\n\n- Build it",
  };
}

function commandRequest(toolCallId: string): PermissionRequest {
  return {
    type: "permission_request",
    toolCallId,
    title: "Allow Codex to run this command?",
    options: COMMAND_OPTIONS,
  };
}

function emitPlanRow(toolCallId: string, text: string): void {
  channelHarness.active?.({
    type: "agent_tool_call",
    toolCallId,
    title: "Plan",
    status: "in_progress",
    kind: "plan",
  });
  channelHarness.active?.({
    type: "agent_tool_update",
    toolCallId,
    status: null,
    text,
    kind: "plan",
  });
}

function emitToolRow(toolCallId: string, title: string, status: string): void {
  channelHarness.active?.({
    type: "agent_tool_call",
    toolCallId,
    title,
    status,
    kind: "execute",
  });
}

function planRowCount(container: HTMLElement): number {
  return container.querySelectorAll(".workspace-chat-tool.is-plan").length;
}

describe("pending plan row suppression", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;
  // The workspace half in miniature: the queue the callbacks fill, the same
  // shape `handlePermissionRequest` / `handlePermissionResolved` leave behind.
  let queue: Array<{ sessionId: string } & RenderedPermissionCard>;
  let onPermissionRequest: (
    sessionId: string,
    subscriptionId: number,
    request: PermissionRequest,
  ) => void;
  let onPermissionResolved: (sessionId: string, resolution: PermissionResolved) => void;

  // Workspace.tsx:1109-1117 in miniature: the pane renders only the first
  // unresolved card of the session, else the resolved head.
  const selectedCard = (): (typeof queue)[number] | null =>
    queue.find((card) => card.sessionId === SESSION && card.resolution === undefined) ??
    queue.find((card) => card.sessionId === SESSION) ??
    null;

  const cardStub = (): ReactNode => {
    const selected = selectedCard();
    if (selected === null) return undefined;
    return (
      <div
        data-testid={selected.request.kind === "plan" ? "permission-plan-card" : "permission-card"}
      />
    );
  };

  const renderSurface = async (): Promise<void> => {
    await act(async () => {
      root.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId={SESSION}
          title="Agent"
          observedState={LIVE}
          pendingPlanToolCallId={pendingPlanId(selectedCard())}
          auxiliary={cardStub()}
          onPermissionRequest={onPermissionRequest}
          onPermissionResolved={onPermissionResolved}
        />,
      );
    });
  };

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    channelHarness.emit = null;
    channelHarness.activeSubscriptionId = null;
    channelHarness.nextSubscriptionId = 41;
    queue = [];
    onPermissionRequest = (_sessionId, _subscriptionId, request) => {
      queue = [...queue, { sessionId: SESSION, request }];
    };
    onPermissionResolved = (_sessionId, resolution) => {
      queue = queue.map((card) =>
        card.sessionId === SESSION &&
        card.request.toolCallId === resolution.toolCallId &&
        card.resolution === undefined
          ? { ...card, resolution: { outcome: "allowed" as const } }
          : card,
      );
    };
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    channelHarness.activeSubscriptionId = null;
    channelHarness.active = null;
    vi.clearAllMocks();
  });

  it("while the plan card waits, the timeline shows no second plan", async () => {
    root = createRoot(container);
    await renderSurface();
    await act(async () => undefined);

    await act(async () => {
      emitPlanRow("plan-1", "## Steps\n\n- Build it");
      channelHarness.active?.(planRequest("plan-1"));
    });
    await renderSurface();

    expect(container.querySelector("[data-testid='permission-plan-card']")).not.toBeNull();
    expect(planRowCount(container)).toBe(0);
  });

  it("when the permission resolves, the row reappears at its original index", async () => {
    root = createRoot(container);
    await renderSurface();
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({ type: "agent_message", messageId: "m1", text: "before" });
      emitPlanRow("plan-1", "## Steps\n\n- Build it");
      emitToolRow("t-after", "cargo test", "completed");
      channelHarness.active?.(planRequest("plan-1"));
    });
    await renderSurface();
    expect(planRowCount(container)).toBe(0);

    await act(async () => {
      channelHarness.active?.({
        type: "permission_resolved",
        toolCallId: "plan-1",
        selectedOptionKind: "allow_once",
      });
    });
    await renderSurface();

    expect(planRowCount(container)).toBe(1);
    const entries = container.querySelectorAll(".workspace-conversation-content > *");
    const kinds = Array.from(entries).map((entry) => entry.className);
    expect(kinds[0]).toContain("workspace-chat-assistant");
    expect(kinds[1]).toContain("is-plan");
    expect(kinds[2]).toContain("workspace-chat-tool");
  });

  it("a plan row stays when the pending permission names a different id", async () => {
    root = createRoot(container);
    await renderSurface();
    await act(async () => undefined);

    await act(async () => {
      emitPlanRow("plan-1", "## Steps\n\n- Build it");
      channelHarness.active?.(planRequest("other-plan"));
    });
    await renderSurface();

    expect(container.querySelector("[data-testid='permission-plan-card']")).not.toBeNull();
    expect(planRowCount(container)).toBe(1);
  });

  it("a plan permission never hides the non-plan row its id names", async () => {
    root = createRoot(container);
    await renderSurface();
    await act(async () => undefined);

    await act(async () => {
      emitToolRow("t-1", "cargo test", "running");
      channelHarness.active?.(planRequest("t-1"));
    });
    await renderSurface();

    expect(container.querySelector("[data-testid='permission-plan-card']")).not.toBeNull();
    expect(container.querySelectorAll(".workspace-chat-tool:not(.is-plan)").length).toBe(1);
  });

  it("a plan whose card waits behind another card keeps its row", async () => {
    root = createRoot(container);
    await renderSurface();
    await act(async () => undefined);

    await act(async () => {
      emitToolRow("t-1", "cargo test", "running");
      // The command card is asked first and stays pending; the plan arrives
      // behind it, so the pane renders the command card only.
      channelHarness.active?.(commandRequest("t-1"));
      emitPlanRow("plan-1", "## Steps\n\n- Build it");
      channelHarness.active?.(planRequest("plan-1"));
    });
    await renderSurface();

    expect(container.querySelector("[data-testid='permission-card']")).not.toBeNull();
    expect(container.querySelector("[data-testid='permission-plan-card']")).toBeNull();
    expect(container.querySelectorAll(".workspace-chat-tool:not(.is-plan)").length).toBe(1);
    expect(planRowCount(container)).toBe(1);
  });
});
