// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import { publishAgentReading, retireAgentReading } from "./agentReadingStore";
import { AgentContextMeter } from "./AgentContextMeter";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const SESSION = "composer-agent";

let host: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  host?.remove();
  host = null;
  root = null;
  retireAgentReading(SESSION);
});

async function render(): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(<AgentContextMeter sessionId={SESSION} />));
  return host;
}

describe("the composer's context meter", () => {
  it("draws nothing until a reading is published for its agent", async () => {
    const host = await render();

    expect(host.querySelector(".workspace-composer-context")).toBeNull();
  });

  it("shows the agent's context beside the composer once a reading arrives", async () => {
    const host = await render();

    await act(async () =>
      publishAgentReading(SESSION, {
        usage: { type: "context_usage", usedTokens: 24_000, maxTokens: 1_000_000, live: true },
        manifest: null,
        lastFinished: null,
        task: null,
      }),
    );

    const meter = host.querySelector(".workspace-composer-context");
    expect(meter?.querySelector(".workspace-context-meter-text")?.textContent).toBe("ctx 24k/1m");
  });
});
