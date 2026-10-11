import { describe, expect, it, vi } from "vitest";
import { sendFirstMessageWhenReady, type FirstMessageAgent } from "./firstMessage";

/** A controller double: its status moves the way the real one's does, and its
 * listeners hear every move. */
function fakeAgent(initial: string) {
  let status = initial;
  const listeners = new Set<() => void>();
  const agent: FirstMessageAgent & { setStatus: (next: string) => void } = {
    getState: () => ({ status }),
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    send: vi.fn(async () => true),
    setStatus: (next) => {
      status = next;
      for (const listener of [...listeners]) listener();
    },
  };
  return { agent, listeners };
}

describe("sendFirstMessageWhenReady", () => {
  it("sends at once when the agent is already ready", () => {
    const { agent } = fakeAgent("idle");

    sendFirstMessageWhenReady(agent, "hello");

    expect(agent.send).toHaveBeenCalledTimes(1);
    expect(agent.send).toHaveBeenCalledWith("hello");
  });

  it("waits through the attach and sends once the agent is idle, never before", () => {
    const { agent, listeners } = fakeAgent("initializing");

    sendFirstMessageWhenReady(agent, "hello");
    expect(agent.send, "a refused send is the thing this waits out").not.toHaveBeenCalled();

    agent.setStatus("running");
    expect(agent.send).not.toHaveBeenCalled();

    agent.setStatus("idle");
    expect(agent.send).toHaveBeenCalledTimes(1);
    expect(listeners.size, "the wait ends with its send").toBe(0);
  });

  it("sends exactly once even when the agent turns idle again later", () => {
    const { agent } = fakeAgent("initializing");

    sendFirstMessageWhenReady(agent, "hello");
    agent.setStatus("idle");
    agent.setStatus("running");
    agent.setStatus("idle");

    expect(agent.send).toHaveBeenCalledTimes(1);
  });

  it("drops the wait when the surface goes away before the agent is ready", () => {
    const { agent, listeners } = fakeAgent("initializing");

    const cancel = sendFirstMessageWhenReady(agent, "hello");
    cancel();
    agent.setStatus("idle");

    expect(agent.send).not.toHaveBeenCalled();
    expect(listeners.size).toBe(0);
  });

  it("cancelling after the send changes nothing", () => {
    const { agent } = fakeAgent("idle");

    const cancel = sendFirstMessageWhenReady(agent, "hello");
    cancel();

    expect(agent.send).toHaveBeenCalledTimes(1);
  });
});
