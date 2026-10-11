// The daemon names the agent an agent message is about: the recipient on an
// outgoing echo, the sender on an incoming delivery. The reducer carries that
// title onto the row, so the card names the agent without a session id.
// @vitest-environment node
import { describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../types/ipc";
import { AgentSession, type AgentChannel, type AgentSessionDeps } from "./agentSession";

function harness() {
  let emit: (event: SessionEvent) => void = () => undefined;
  const invoke = vi.fn(async (command: string) =>
    command === "session_attach" ? 41 : command === "session_send" ? true : undefined,
  ) as unknown as AgentSessionDeps["invoke"];
  const session = new AgentSession({
    sessionId: "agent-1",
    invoke,
    createChannel: (onEvent) => {
      emit = onEvent;
      return {} as AgentChannel;
    },
  });
  return { session, emit: (event: SessionEvent) => emit(event) };
}

describe("agent message rows carry the daemon's title", () => {
  it("names the recipient on the outgoing echo", async () => {
    const { session, emit } = harness();
    await session.start();
    emit({
      type: "agent_user_message",
      author: "agent",
      messageId: "m-out",
      messageKind: "outgoing_a2a",
      text: "please check the hero",
      peerTitle: "Finish Kalsa website",
    });
    const row = session.getState().items.at(-1);
    expect(row).toMatchObject({
      role: "a2a_outgoing_message",
      peerTitle: "Finish Kalsa website",
    });
  });

  it("names the sender on an incoming delivery", async () => {
    const { session, emit } = harness();
    await session.start();
    emit({
      type: "agent_user_message",
      author: "agent",
      messageId: "m-in",
      messageKind: "incoming_a2a",
      text: [
        "<devboule-system>",
        "origin: local",
        "role: client",
        "from_agent: s.kalsa.finish",
        "timestamp: 1789671600000",
        "Hero image confirmed.",
        "</devboule-system>",
      ].join("\n"),
      peerTitle: "Finish Kalsa website",
    });
    const row = session.getState().items.at(-1);
    expect(row).toMatchObject({ role: "a2a_message", peerTitle: "Finish Kalsa website" });
  });

  it("leaves the title off a row whose event carried none", async () => {
    const { session, emit } = harness();
    await session.start();
    emit({
      type: "agent_user_message",
      author: "agent",
      messageId: "m-out-2",
      messageKind: "outgoing_a2a",
      text: "no name here",
    });
    const row = session.getState().items.at(-1);
    expect(row).toBeDefined();
    expect(row && "peerTitle" in row).toBe(false);
  });
});
