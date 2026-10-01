// @vitest-environment happy-dom

// The agreement contract at the component level: for real roster rows the
// rendered header shows exactly what the chip shows — a painted dot, a
// non-empty word, the shared tooltip — so a header that renders nothing for
// a state fails here, not in a screenshot.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import type { Session, SessionState } from "../../../types/ipc";
import { chipDisplay } from "../strip/stripDisplay";
import { PaneHeader } from "./PaneHeader";
import { headerMenu } from "./paneHeaderMenu";
import { headerDisplay } from "./paneHeaderStatus";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const RECOVERED: SessionState = {
  type: "recovered",
  generation: 2,
  integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
};
const ENDED: SessionState = {
  type: "ended",
  generation: 1,
  code: 1,
  integrity: { kind: "complete" },
};

function cleanRow(state: SessionState): Session {
  return { state, elapsedMs: null } as unknown as Session;
}

async function renderHeaderFor(
  state: SessionState,
  agentStatus: "idle" | "error" | "closed",
): Promise<void> {
  const display = headerDisplay(state, null, agentStatus, undefined, undefined);
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(
      <PaneHeader
        kind="agent"
        title="Agent"
        display={display}
        menu={headerMenu("C:\\x", undefined)}
      />,
    );
  });
}

afterEach(() => {
  document.body.replaceChildren();
});

describe("the rendered header agrees with the chip", () => {
  it("shows ring dot plus Recovered for a recovered row the controller failed on", async () => {
    const chip = chipDisplay(cleanRow(RECOVERED));
    await renderHeaderFor(RECOVERED, "error");
    const header = document.querySelector(".workspace-agent-toolbar");
    if (header === null) throw new Error("pane header did not render");
    const dot = header.querySelector(".workspace-status-dot");
    expect(dot?.className).toContain("workspace-dot-recovered");
    expect(dot?.className).not.toContain("undefined");
    const status = header.querySelector('[role="status"]');
    const visible = status?.childNodes[0]?.textContent ?? "";
    expect(visible.length).toBeGreaterThan(0);
    expect(visible).toBe("Recovered");
    expect(status?.getAttribute("title")).toBe(chip.tooltip);
    expect(status?.querySelector(".sr-only")?.textContent).toBe(
      " — restored after the restart; some messages could not be checked.",
    );
  });

  it("shows the ended word with a painted dot for an ended row", async () => {
    const chip = chipDisplay(cleanRow(ENDED));
    await renderHeaderFor(ENDED, "closed");
    const header = document.querySelector(".workspace-agent-toolbar");
    if (header === null) throw new Error("pane header did not render");
    const dot = header.querySelector(".workspace-status-dot");
    expect(dot?.className).toContain("workspace-dot-terracotta");
    expect(dot?.className).not.toContain("undefined");
    const status = header.querySelector('[role="status"]');
    const visible = status?.childNodes[0]?.textContent ?? "";
    expect(visible.length).toBeGreaterThan(0);
    expect(visible).toBe("Stopped");
    expect(status?.getAttribute("title")).toBe(chip.tooltip);
  });
});
