// Composer image references ride the user-message echo onto the chat item,
// so the transcript can resolve them after a replay. Rows written before
// references existed render exactly as before.
import { describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../types/ipc";
import type { AttachmentReference } from "./tauri";

const historyMocks = vi.hoisted(() => ({ recordChildFinishedHistory: vi.fn(async () => true) }));
const mirrorMocks = vi.hoisted(() => ({ scheduleDelegatedDesignMirror: vi.fn() }));

vi.mock("../features/design/childFinishedHistory", () => ({
  recordChildFinishedHistory: historyMocks.recordChildFinishedHistory,
}));
vi.mock("../features/design/delegatedDesignMirror", () => ({
  scheduleDelegatedDesignMirror: mirrorMocks.scheduleDelegatedDesignMirror,
}));

import { AgentSession, type AgentChannel, type AgentSessionDeps } from "./agentSession";

function makeHarness() {
  let emit: (event: SessionEvent) => void = () => undefined;
  const invoke = vi.fn(async (command: string, _args?: Record<string, unknown>) =>
    command === "session_attach" ? 41 : undefined,
  ) as unknown as AgentSessionDeps["invoke"];
  const deps: AgentSessionDeps = {
    sessionId: "agent-1",
    invoke,
    createChannel: (onEvent) => {
      emit = onEvent;
      return {} as AgentChannel;
    },
  };
  return { session: new AgentSession(deps), emit: (event: SessionEvent) => emit(event) };
}

const REF: AttachmentReference = {
  sessionId: "agent-1",
  digest: "c".repeat(64),
  storedBytes: 56,
};

describe("agent session chat images", () => {
  it("carries the echo's image references onto the user item", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.emit({
      type: "agent_user_message",
      messageId: "user-1",
      text: "look at this",
      author: "human",
      messageKind: "composer",
      images: [REF],
    });
    const items = harness.session.getState().items;
    expect(items).toHaveLength(1);
    const item = items[0]!;
    expect(item.role).toBe("user");
    if (item.role !== "user") throw new Error("expected a user item");
    expect(item.images).toEqual([REF]);
  });

  it("keeps a text-only echo free of the images key", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.emit({
      type: "agent_user_message",
      messageId: "user-2",
      text: "plain prompt",
      author: "human",
      messageKind: "composer",
    });
    const item = harness.session.getState().items[0]!;
    expect(item.role).toBe("user");
    if (item.role !== "user") throw new Error("expected a user item");
    expect(item.images).toBeUndefined();
    expect("images" in item).toBe(false);
  });

  it("replays a recovered image echo onto the same user item", async () => {
    const harness = makeHarness();
    await harness.session.start();
    const echo: SessionEvent = {
      type: "agent_user_message",
      messageId: "user-3",
      text: "look at this",
      author: "human",
      messageKind: "composer",
      images: [REF],
    };
    harness.emit(echo);
    harness.emit(echo);
    const users = harness.session.getState().items.filter((item) => item.role === "user");
    expect(users).toHaveLength(1);
    if (users[0]!.role !== "user") throw new Error("expected a user item");
    expect(users[0]!.images).toEqual([REF]);
  });

  it("keeps a row's own words verbatim when it carries references", async () => {
    const harness = makeHarness();
    await harness.session.start();
    harness.emit({
      type: "agent_user_message",
      messageId: "user-new",
      text: "look at this\n\n[Image available at: C:\\typed\\by\\hand.png]",
      author: "human",
      messageKind: "composer",
      images: [REF],
    });
    const item = harness.session.getState().items[0]!;
    expect(item.role).toBe("user");
    if (item.role !== "user") throw new Error("expected a user item");
    expect(item.text).toBe("look at this\n\n[Image available at: C:\\typed\\by\\hand.png]");
  });
});
