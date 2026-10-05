// The finished turn's metadata line: model, stop reason and cost at a glance,
// the token accounting behind one disclosure. The line itself stays a line —
// nothing here may reflow the transcript when a value appears or disappears.
// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AgentFinished } from "../../../lib/agentSession";
import { TurnFooter } from "./TurnFooter";

const FINISHED: AgentFinished = {
  stopReason: "error",
  modelId: "grok-4.6",
  usage: {
    inputTokens: 20753,
    outputTokens: 30,
    totalTokens: 20783,
    cacheReadTokens: 6016,
    cacheWriteTokens: 0,
    costUsd: 0.00555254,
  },
};

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

async function render(finished: AgentFinished | null): Promise<void> {
  await act(async () => root.render(<TurnFooter finished={finished} />));
}

function disclosure(): HTMLDetailsElement | null {
  return container.querySelector<HTMLDetailsElement>(".turn-footer-detail");
}

function trigger(): HTMLElement | null {
  return container.querySelector<HTMLElement>(".turn-footer-detail > summary");
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

describe("the turn footer's short line", () => {
  it("takes no row at all for a normally finished turn", async () => {
    await render({ ...FINISHED, stopReason: "end_turn" });
    expect(container.textContent).toBe("");

    await render({ ...FINISHED, stopReason: "completed" });
    expect(container.textContent).toBe("");
  });

  it("names the model, the stop reason and the cost, and nothing else", async () => {
    await render(FINISHED);

    expect(container.querySelector(".turn-footer-line")?.textContent).toBe(
      "model grok-4.6 · stopped: error · $0.0055",
    );
    // The accounting sits in the disclosure, not in the line at a glance.
    expect(container.querySelector(".turn-footer-line")?.textContent).not.toContain("20,753");
  });

  it("keeps a cancelled or maxed-out turn's row", async () => {
    await render({ stopReason: "cancelled" });
    expect(container.querySelector(".turn-footer-line")?.textContent).toBe("stopped: cancelled");

    await render({ stopReason: "max_tokens" });
    expect(container.querySelector(".turn-footer-line")?.textContent).toBe("stopped: max_tokens");
  });

  it("omits a value the daemon did not send rather than printing an empty one", async () => {
    await render({ stopReason: "error" });

    expect(container.querySelector(".turn-footer-line")?.textContent).toBe("stopped: error");
  });

  it("renders nothing at all for a turn that has not finished", async () => {
    await render(null);

    expect(container.textContent).toBe("");
  });

  it("prints no cost figure for a cost of zero", async () => {
    await render({ stopReason: "error", usage: { costUsd: 0 } });

    expect(container.querySelector(".turn-footer-line")?.textContent).toBe("stopped: error");
  });
});

describe("the turn footer's token disclosure", () => {
  it("starts closed, with the ledger inside it", async () => {
    await render(FINISHED);

    expect(disclosure()?.open).toBe(false);
    expect(disclosure()?.querySelector(".turn-footer-detail-copy")).not.toBeNull();
  });

  it("lists every token and cache figure once opened", async () => {
    await render(FINISHED);

    const details = disclosure();
    if (details === null) throw new Error("the turn footer rendered no disclosure");
    await act(async () => {
      details.open = true;
    });

    expect(details.querySelector(".turn-footer-detail-copy")?.textContent).toBe(
      "in 20,753 · out 30 · cached 6,016 · cache-wrote 0 · total 20,783 tokens",
    );
  });

  it("is reachable and operable from the keyboard alone", async () => {
    await render(FINISHED);

    const summary = trigger();
    if (summary === null) throw new Error("the turn footer rendered no trigger");
    // A <summary> is the disclosure itself: browsers put it in the tab order
    // and toggle it on Enter and Space, with no key handler here. happy-dom
    // reports tabIndex -1 for it, so the reach is asserted by focus().
    expect(summary.tagName).toBe("SUMMARY");
    summary.focus();
    expect(document.activeElement).toBe(summary);
    // The control names what it opens, so the collapsed line is not a dead end.
    expect(summary.getAttribute("aria-label")).toBe("Turn token detail");
  });

  it("offers no disclosure when the daemon sent no usage", async () => {
    await render({ stopReason: "error", modelId: "grok" });

    expect(disclosure()).toBeNull();
  });
});
