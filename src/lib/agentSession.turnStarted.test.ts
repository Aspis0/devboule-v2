// What a caller can ask the controller before it acts on a fresh session: whether
// a hot switch can go out, whether the session has worked, and when the first
// turn begins.
// @vitest-environment node
import { describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../types/ipc";
import { AgentSession, type AgentChannel, type AgentSessionDeps } from "./agentSession";

function harness(onTurnStarted?: () => void) {
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
    ...(onTurnStarted === undefined ? {} : { onTurnStarted }),
  });
  return { session, emit: (event: SessionEvent) => emit(event) };
}

describe("the first turn of a session", () => {
  it("is announced by the send itself, before the status reads running", async () => {
    const statuses: string[] = [];
    const ref: { session?: AgentSession } = {};
    const made = harness(() => statuses.push(ref.session?.getState().status ?? "unbuilt"));
    ref.session = made.session;
    await made.session.start();

    const sent = made.session.send("Say hello");
    expect(statuses).toEqual(["idle"]);
    expect(made.session.getState().status).toBe("running");
    await sent;
  });

  it("is announced by a frame of a turn this view did not send", async () => {
    const onTurnStarted = vi.fn();
    const { session, emit } = harness(onTurnStarted);
    await session.start();
    expect(onTurnStarted).not.toHaveBeenCalled();

    emit({ type: "agent_message", messageId: "m-1", text: "Hello" });

    expect(onTurnStarted).toHaveBeenCalledTimes(1);
  });

  it("is not announced by a manifest", async () => {
    const onTurnStarted = vi.fn();
    const { session, emit } = harness(onTurnStarted);
    await session.start();

    emit({ type: "session_manifest", providerId: "claude", currentModelId: "sonnet", models: [] });

    expect(onTurnStarted).not.toHaveBeenCalled();
  });
});

describe("whether a session has worked", () => {
  const incomingA2a: SessionEvent = {
    type: "agent_user_message",
    author: "agent",
    messageId: "m-incoming",
    text: [
      "<devboule-system>",
      "origin: local",
      "role: client",
      "from_agent: s.msg.source",
      "timestamp: 1789671600000",
      "words received from another agent",
      "</devboule-system>",
    ].join("\n"),
    messageKind: "incoming_a2a",
  };

  it("is no for a session that holds only its creation notice", async () => {
    const { session, emit } = harness();
    await session.start();
    expect(session.hasWorked()).toBe(false);

    emit({
      type: "agent_user_message",
      author: "creation",
      messageId: "m-creation",
      text: "standing instructions",
      messageKind: "creation",
    });

    expect(session.hasWorked()).toBe(false);
  });

  it("is yes once a peer's message is on the transcript, though no turn opened here", async () => {
    const onTurnStarted = vi.fn();
    const { session, emit } = harness(onTurnStarted);
    await session.start();

    emit(incomingA2a);

    expect(onTurnStarted).not.toHaveBeenCalled();
    expect(session.getState().status).toBe("idle");
    expect(session.hasWorked()).toBe(true);
  });

  it("is yes while a turn this view sent is running", async () => {
    const { session } = harness();
    await session.start();

    const sent = session.send("Say hello");

    expect(session.hasWorked()).toBe(true);
    await sent;
  });
});

describe("whether a hot switch can go out", () => {
  it("is no until the session has attached, yes after, and no once it is disposed", async () => {
    const { session } = harness();
    expect(session.canSwitch()).toBe(false);

    await session.start();
    expect(session.canSwitch()).toBe(true);

    session.dispose();
    expect(session.canSwitch()).toBe(false);
  });
});
