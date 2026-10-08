import { afterEach, expect, it, vi } from "vitest";
import { acquire, resetRegistry, syncOpenTabs, tasksNews } from "./agentSessionRegistry";
import type { AgentChannel, AgentSessionDeps } from "./agentSession";
import type { SessionTask } from "../types/ipc";

function child(state: SessionTask["state"]): SessionTask {
  return {
    id: "child",
    title: "Explore",
    kind: "agent",
    state,
    sessionId: "remembered",
    startedAtMs: 1,
  };
}

function deps(tasks: SessionTask[], daemonId: string): AgentSessionDeps {
  return {
    sessionId: "remembered",
    daemonEpoch: () => daemonId,
    invoke: vi.fn(async (cmd) => {
      if (cmd === "session_attach") return 41;
      if (cmd === "session_tasks") return { tasks, omitted: 0 };
      return undefined;
    }) as AgentSessionDeps["invoke"],
    createChannel: () => ({}) as AgentChannel,
  };
}

const settle = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

afterEach(() => resetRegistry());

it("compares a re-created entry's first list with the list the previous one showed", async () => {
  const first = acquire(deps([child("running")], "daemon-1"), 1);
  await first.session.start();
  await settle();
  syncOpenTabs([]);

  const second = acquire(deps([child("finished")], "daemon-1"), 1);
  await second.session.start();
  await settle();

  expect(tasksNews(second.session)).toBe(true);
});

it("treats the first list of a changed daemon as a baseline, not news", async () => {
  const first = acquire(deps([child("running")], "daemon-1"), 1);
  await first.session.start();
  await settle();
  syncOpenTabs([]);

  const second = acquire(deps([child("finished")], "daemon-2"), 1);
  await second.session.start();
  await settle();

  expect(tasksNews(second.session)).toBe(false);
});
