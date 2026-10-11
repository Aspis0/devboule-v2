import { describe, expect, it, vi } from "vitest";
import { sendFirstMessageWhenReady, sendWhenReady, type FirstMessageAgent } from "./firstMessage";

/** A controller double: its status moves the way the real one's does, and its
 * listeners hear every move. `answer` is what its send resolves with. */
function fakeAgent(initial: string, answer: boolean | Error = true) {
  let status = initial;
  const listeners = new Set<() => void>();
  const agent: FirstMessageAgent & { setStatus: (next: string) => void } = {
    getState: () => ({ status }),
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    send: vi.fn(async () => {
      if (answer instanceof Error) throw answer;
      return answer;
    }),
    setStatus: (next) => {
      status = next;
      for (const listener of [...listeners]) listener();
    },
  };
  return { agent, listeners };
}

describe("sendFirstMessageWhenReady", () => {
  it("sends at once when the agent is already ready, and reports the agent's answer", async () => {
    const { agent } = fakeAgent("idle");
    const onSettled = vi.fn();

    sendFirstMessageWhenReady(agent, "hello", onSettled);
    await vi.waitFor(() => expect(onSettled).toHaveBeenCalledWith(true));

    expect(agent.send).toHaveBeenCalledTimes(1);
    expect(agent.send).toHaveBeenCalledWith("hello");
  });

  it("waits through the attach and sends once the agent is idle, never before", async () => {
    const { agent, listeners } = fakeAgent("initializing");
    const onSettled = vi.fn();

    sendFirstMessageWhenReady(agent, "hello", onSettled);
    expect(agent.send, "a refused send is the thing this waits out").not.toHaveBeenCalled();

    agent.setStatus("running");
    expect(agent.send).not.toHaveBeenCalled();

    agent.setStatus("idle");
    await vi.waitFor(() => expect(onSettled).toHaveBeenCalledWith(true));
    expect(agent.send).toHaveBeenCalledTimes(1);
    expect(listeners.size, "the wait ends with its send").toBe(0);
  });

  it("reports a refusal, so the text is never taken as received", async () => {
    const { agent } = fakeAgent("idle", false);
    const onSettled = vi.fn();

    sendFirstMessageWhenReady(agent, "hello", onSettled);
    await vi.waitFor(() => expect(onSettled).toHaveBeenCalledWith(false));
  });

  it("reports a thrown send as not sent", async () => {
    const { agent } = fakeAgent("idle", new Error("link down"));
    const onSettled = vi.fn();

    sendFirstMessageWhenReady(agent, "hello", onSettled);
    await vi.waitFor(() => expect(onSettled).toHaveBeenCalledWith(false));
  });

  it("sends exactly once even when the agent turns idle again later", async () => {
    const { agent } = fakeAgent("initializing");
    const onSettled = vi.fn();

    sendFirstMessageWhenReady(agent, "hello", onSettled);
    agent.setStatus("idle");
    agent.setStatus("running");
    agent.setStatus("idle");
    await vi.waitFor(() => expect(onSettled).toHaveBeenCalledTimes(1));

    expect(agent.send).toHaveBeenCalledTimes(1);
  });

  it("drops the wait when the surface goes away before the agent is ready, reporting nothing", () => {
    const { agent, listeners } = fakeAgent("initializing");
    const onSettled = vi.fn();

    const cancel = sendFirstMessageWhenReady(agent, "hello", onSettled);
    cancel();
    agent.setStatus("idle");

    expect(agent.send).not.toHaveBeenCalled();
    expect(onSettled).not.toHaveBeenCalled();
    expect(listeners.size).toBe(0);
  });
});

describe("sendWhenReady", () => {
  it("never attempts a send to an agent that is not ready", async () => {
    const { agent } = fakeAgent("running");

    await expect(sendWhenReady(agent, "again")).resolves.toBe(false);
    expect(agent.send).not.toHaveBeenCalled();
  });

  it("answers with the agent's answer when it is ready", async () => {
    const { agent } = fakeAgent("idle", true);

    await expect(sendWhenReady(agent, "again")).resolves.toBe(true);
  });
});
