// @vitest-environment happy-dom

// A compaction is a divider of its own: a spinner while the context compacts,
// then "Context compacted" once it is done. The daemon's own words decide which.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { compactionState } from "./compactionState";
import { TranscriptRow } from "./TranscriptRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root | null = null;

function notice(text: string): AgentChatItem {
  return { id: "system-1", role: "system", text, severity: "info" };
}

async function renderNotice(text: string): Promise<HTMLElement> {
  root = createRoot(container);
  await act(async () => {
    root?.render(
      <TranscriptRow
        entry={notice(text)}
        fileLinks={null}
        transcriptEnded={false}
        isStreamingThought={false}
      />,
    );
  });
  return container;
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.append(container);
});

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  container.remove();
});

describe("compaction notices", () => {
  it("maps the daemon's start words to a loading state and its finish words to done", () => {
    expect(compactionState("Compacting the context.")).toBe("loading");
    expect(compactionState("Context compacted.")).toBe("done");
    expect(compactionState("Context manually compacted")).toBe("done");
    expect(compactionState("Compacted")).toBe("done");
    expect(compactionState("The model said hello.")).toBeNull();
  });

  it("draws a divider with a spinner while the context compacts", async () => {
    const shown = await renderNotice("Compacting the context.");
    const divider = shown.querySelector(".workspace-chat-compaction");
    expect(divider?.getAttribute("data-state")).toBe("loading");
    expect(divider?.getAttribute("role")).toBe("status");
    expect(divider?.querySelector(".workspace-chat-compaction-label")?.textContent).toBe(
      "Compacting context…",
    );
    expect(divider?.querySelector(".workspace-chat-compaction-spinner")).not.toBeNull();
  });

  it("draws the same divider as done, with no spinner, once the context is compacted", async () => {
    const shown = await renderNotice("Context compacted.");
    const divider = shown.querySelector(".workspace-chat-compaction");
    expect(divider?.getAttribute("data-state")).toBe("done");
    expect(divider?.querySelector(".workspace-chat-compaction-label")?.textContent).toBe(
      "Context compacted",
    );
    expect(divider?.querySelector(".workspace-chat-compaction-spinner")).toBeNull();
  });

  it("says a start whose turn ended did not finish, with no spinner, once it is not live", async () => {
    expect(compactionState("Compacting the context.", false)).toBe("unfinished");
    root = createRoot(container);
    await act(async () => {
      root?.render(
        <TranscriptRow
          entry={notice("Compacting the context.")}
          fileLinks={null}
          transcriptEnded={false}
          isStreamingThought={false}
          compactionLive={false}
        />,
      );
    });
    const divider = container.querySelector(".workspace-chat-compaction");
    expect(divider?.getAttribute("data-state")).toBe("unfinished");
    expect(divider?.querySelector(".workspace-chat-compaction-label")?.textContent).toBe(
      "Compaction did not finish",
    );
    expect(divider?.querySelector(".workspace-chat-compaction-spinner")).toBeNull();
  });

  it("reads a lone manual compaction as done too", async () => {
    const shown = await renderNotice("Context manually compacted");
    expect(shown.querySelector(".workspace-chat-compaction-label")?.textContent).toBe(
      "Context compacted",
    );
  });

  it("leaves an ordinary system line as a plain line", async () => {
    const shown = await renderNotice("The model said hello.");
    expect(shown.querySelector(".workspace-chat-compaction")).toBeNull();
    expect(shown.textContent).toContain("The model said hello.");
  });
});
