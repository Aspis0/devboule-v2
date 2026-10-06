// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import type { ContextUsage } from "../../../types/ipc";
import { AgentReadingPublisher, type UsageSource } from "./AgentReadingPublisher";
import { publishAgentReading, retireAgentReading, useAgentReading } from "./agentReadingStore";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement | null = null;
let root: ReturnType<typeof createRoot> | null = null;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  host?.remove();
  root = null;
  host = null;
});

function Reader({
  sessionId,
  onRead,
}: {
  sessionId: string;
  onRead: (task: string | null | undefined) => void;
}) {
  onRead(useAgentReading(sessionId)?.task);
  return null;
}

describe("publishing an agent's reading", () => {
  it("publishes from the session's usage lane alone, and retires the reading with the surface", async () => {
    let notifyUsage: (() => void) | null = null;
    let stored: ContextUsage | null = null;
    const source: UsageSource = {
      subscribeUsage(listener) {
        notifyUsage = listener;
        return () => {
          notifyUsage = null;
        };
      },
      getContextUsage: () => stored,
    };
    const seen: Array<ContextUsage | null> = [];
    function Probe() {
      seen.push(useAgentReading("pub-1")?.usage ?? null);
      return null;
    }
    host = document.createElement("div");
    document.body.append(host);
    root = createRoot(host);
    await act(async () =>
      root?.render(
        <>
          <AgentReadingPublisher
            sessionId="pub-1"
            session={source}
            manifest={null}
            lastFinished={null}
            task="running tests"
          />
          <Probe />
        </>,
      ),
    );
    expect(seen.at(-1)).toBeNull();

    await act(async () => {
      stored = { type: "context_usage", usedTokens: 76_000, maxTokens: 200_000, live: true };
      notifyUsage?.();
    });
    expect(seen.at(-1)?.usedTokens).toBe(76_000);

    await act(async () => root?.render(<Probe />));
    expect(seen.at(-1)).toBeNull();
  });
});

describe("the reading store", () => {
  it("gives a session nothing until a surface publishes, and nothing again once it retires", async () => {
    const tasks: Array<string | null | undefined> = [];
    host = document.createElement("div");
    document.body.append(host);
    root = createRoot(host);
    await act(async () =>
      root?.render(<Reader sessionId="store-1" onRead={(task) => tasks.push(task)} />),
    );
    expect(tasks.at(-1)).toBeUndefined();

    await act(async () =>
      publishAgentReading("store-1", {
        usage: null,
        manifest: null,
        lastFinished: null,
        task: "editing",
      }),
    );
    expect(tasks.at(-1)).toBe("editing");

    await act(async () => retireAgentReading("store-1"));
    expect(tasks.at(-1)).toBeUndefined();
  });
});
