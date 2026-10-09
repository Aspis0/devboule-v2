// @vitest-environment happy-dom

// A finished turn's row carries no words of its own: the stop reason, the
// tokens and the cost live behind one disclosure, and a normal end draws no
// line at all.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import type { AgentFinished } from "../../../lib/agentSession";
import { usdCopy } from "../../../lib/format";
import { TurnFooter } from "./TurnFooter";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const LEDGER = {
  inputTokens: 1200,
  outputTokens: 340,
  cacheReadTokens: 800,
  costUsd: 0.0022,
};

function finished(overrides: Partial<AgentFinished> = {}): AgentFinished {
  return { stopReason: "stop", usage: LEDGER, ...overrides } as AgentFinished;
}

let root: ReturnType<typeof createRoot> | null = null;
let host: HTMLDivElement | null = null;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  host?.remove();
  host = null;
});

async function renderFooter(
  value: AgentFinished | null,
  providerId?: string,
): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(<TurnFooter finished={value} providerId={providerId} />));
  return host;
}

async function openDetail(container: HTMLElement): Promise<string> {
  const details = container.querySelector("details");
  if (details === null) throw new Error("the turn had no disclosure");
  await act(async () => {
    details.open = true;
  });
  return details.querySelector(".turn-footer-detail-copy")?.textContent ?? "";
}

describe("TurnFooter", () => {
  it("draws no stop line for a normal end, only the disclosure", async () => {
    const container = await renderFooter(finished());
    expect(container.querySelector(".turn-footer-line")).toBeNull();
    expect(container.textContent).not.toContain("stopped");
    expect(container.querySelector(".turn-footer-detail")).not.toBeNull();
  });

  it("keeps the turn's cost and tokens inside the disclosure", async () => {
    const container = await renderFooter(finished(), "claude");
    const copy = await openDetail(container);
    expect(copy).toContain("in 1,200");
    expect(copy).toContain("out 340");
    expect(copy).toContain(usdCopy(0.0022));
  });

  it("moves an abnormal stop reason into the disclosure, not onto the row", async () => {
    const container = await renderFooter(finished({ stopReason: "length" }));
    expect(container.querySelector(".turn-footer-line")).toBeNull();
    const copy = await openDetail(container);
    expect(copy).toContain("stopped: length");
  });

  it("names the disclosure for everything it can hold, not only tokens", async () => {
    const container = await renderFooter(finished());
    expect(container.querySelector("summary")?.getAttribute("aria-label")).toBe("Turn details");
  });

  it("draws nothing for a normal end that carried no usage", async () => {
    const container = await renderFooter(finished({ usage: undefined }));
    expect(container.querySelector(".turn-footer")).toBeNull();
    expect(container.textContent).toBe("");
  });

  it("takes no cost for a pi turn, whatever the figure", async () => {
    const container = await renderFooter(finished(), "pi");
    const copy = await openDetail(container);
    expect(copy).not.toContain("$");
    expect(copy).toContain("in 1,200");
  });
});
