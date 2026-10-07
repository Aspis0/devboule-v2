// @vitest-environment happy-dom

// The pane status speaks its state word once: the visible label plus the
// screen-reader suffix must not repeat it, the supplemental detail must
// survive, and the hover tooltip keeps the full line. Both pane branches
// render the same status node, so every case runs for agent and terminal.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import type { AgentStatus } from "../../../lib/agentSession";
import type { Attention, SessionState } from "../../../types/ipc";
import { PaneHeader } from "./PaneHeader";
import { headerMenu } from "./paneHeaderMenu";
import { headerDisplay, type HeaderDisplay } from "./paneHeaderStatus";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const LIVE: SessionState = { type: "live", generation: 1 };
const SILENT: SessionState = { type: "silent", generation: 1 };
const RECOVERED: SessionState = {
  type: "recovered",
  generation: 2,
  integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
};
// A recovered row that reported no integrity block: the roster line then
// ends at the restoration sentence, with no tail after it.
const RECOVERED_NO_INTEGRITY = { type: "recovered", generation: 2 } as unknown as SessionState;
const ENDED: SessionState = {
  type: "ended",
  generation: 1,
  code: 1,
  integrity: { kind: "complete" },
};
const ENDED_TRUNCATED: SessionState = {
  type: "ended",
  generation: 1,
  code: 1,
  integrity: { kind: "truncated", droppedFrames: 1, droppedBytes: 1, trimmedBytes: 1 },
};
// A state outside the roster vocabulary: the header falls back to its
// Connecting word over the roster's own Status unknown line.
const UNRECOGNIZED = { type: "unrecognized" } as unknown as SessionState;

interface StatusCase {
  name: string;
  observed: SessionState | null;
  elapsedMs: number | null;
  status: AgentStatus | null;
  attention?: Attention;
  /** The state word the announcement must say exactly once. */
  word: string;
  /** The visible label, compact detail included. */
  visible: string;
  /** The screen-reader suffix after the word; null omits it entirely. */
  suffix: string | null;
  /** The unchanged hover tooltip. */
  tooltip: string;
}

const CASES: StatusCase[] = [
  {
    name: "live",
    observed: LIVE,
    elapsedMs: 0,
    status: "idle",
    word: "Running",
    visible: "Running",
    suffix: null,
    tooltip: "Running",
  },
  {
    name: "silent with a compact detail",
    observed: SILENT,
    elapsedMs: 240_000,
    status: "idle",
    word: "Quiet",
    visible: "Quiet · 4 m",
    suffix: "no output for 4 minutes, may still be working.",
    tooltip: "Quiet — no output for 4 minutes, may still be working.",
  },
  {
    name: "silent without a compact detail",
    observed: SILENT,
    elapsedMs: null,
    status: "idle",
    word: "Quiet",
    visible: "Quiet",
    suffix: "no output, may still be working.",
    tooltip: "Quiet — no output, may still be working.",
  },
  {
    name: "recovered with the integrity tail",
    observed: RECOVERED,
    elapsedMs: null,
    status: "idle",
    word: "Recovered",
    visible: "Recovered",
    suffix: "restored after the restart; some messages could not be checked.",
    tooltip: "Recovered — restored after the restart; some messages could not be checked.",
  },
  {
    name: "recovered without an integrity tail",
    observed: RECOVERED_NO_INTEGRITY,
    elapsedMs: null,
    status: "idle",
    word: "Recovered",
    visible: "Recovered",
    suffix: "restored after the restart",
    tooltip: "Recovered — restored after the restart",
  },
  {
    name: "ended",
    observed: ENDED,
    elapsedMs: null,
    status: "idle",
    word: "Stopped",
    visible: "Stopped",
    suffix: null,
    tooltip: "Stopped",
  },
  {
    name: "ended with an integrity tail",
    observed: ENDED_TRUNCATED,
    elapsedMs: null,
    status: "idle",
    word: "Stopped",
    visible: "Stopped",
    suffix: "the end is missing.",
    tooltip: "Stopped; the end is missing.",
  },
  {
    name: "controller error on a still-up row",
    observed: LIVE,
    elapsedMs: 0,
    status: "error",
    word: "Failed",
    visible: "Failed",
    suffix: null,
    tooltip: "Failed",
  },
  {
    name: "closed controller on a still-up row",
    observed: LIVE,
    elapsedMs: 0,
    status: "closed",
    word: "Stopped",
    visible: "Stopped",
    suffix: null,
    tooltip: "Stopped",
  },
  {
    name: "no observed row",
    observed: null,
    elapsedMs: null,
    status: "idle",
    word: "Connecting",
    visible: "Connecting",
    suffix: null,
    tooltip: "Connecting",
  },
  {
    name: "unrecognized state",
    observed: UNRECOGNIZED,
    elapsedMs: null,
    status: "idle",
    word: "Connecting",
    visible: "Connecting",
    suffix: "Status unknown",
    tooltip: "Status unknown",
  },
  {
    name: "permission attention over a live row",
    observed: LIVE,
    elapsedMs: 0,
    status: "idle",
    attention: { reason: "permission", atMs: 1 },
    word: "Needs your approval",
    visible: "Needs your approval",
    suffix: "Running",
    tooltip: "Running\nNeeds your approval",
  },
];

async function renderHeader(display: HeaderDisplay): Promise<void> {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(
      <PaneHeader title="zsh · api" display={display} menu={headerMenu("C:\\x", undefined)} />,
    );
  });
}

afterEach(() => {
  document.body.replaceChildren();
});

describe("the terminal pane status", () => {
  const toolbar = ".workspace-terminal-toolbar";

  for (const c of CASES) {
    it(`announces ${c.name} once, with the detail and the title intact`, async () => {
      const display = headerDisplay(c.observed, c.elapsedMs, c.status, undefined, c.attention);
      await renderHeader(display);

      const status = document.querySelector(toolbar)?.querySelector('[role="status"]');
      if (status === null || status === undefined) throw new Error("pane status did not render");
      expect(status.getAttribute("aria-label")).toBeNull();

      const spoken = c.suffix === null ? c.visible : `${c.visible} — ${c.suffix}`;
      expect(status.textContent).toBe(spoken);
      expect(status.textContent?.split(c.word).length - 1).toBe(1);
      if (c.suffix !== null) expect(status.textContent).toContain(c.suffix);
      expect(status.getAttribute("title")).toBe(c.tooltip);

      const suffix = status.querySelector(".sr-only");
      if (c.suffix === null) {
        expect(suffix).toBeNull();
      } else {
        expect(suffix?.textContent).toBe(` — ${c.suffix}`);
      }
    });
  }
});
