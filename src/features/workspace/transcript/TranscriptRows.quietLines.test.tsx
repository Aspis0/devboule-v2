// @vitest-environment happy-dom

// Each row kind reads as one quiet line; its detail waits behind the click.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { TranscriptRows } from "./TranscriptRows";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const names = { sessionById: new Map(), deviceNames: new Map() };

let root: Root | null = null;
let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  host.remove();
});

async function show(items: AgentChatItem[]): Promise<void> {
  await act(async () =>
    root?.render(
      <TranscriptRows
        items={items}
        recoveredAttach={false}
        a2aNames={names}
        fileLinks={null}
        transcriptEnded={false}
        streamingThoughtId={null}
      />,
    ),
  );
}

function call(title: string, status: string, output = ""): AgentChatItem {
  return {
    id: `tool-${title}`,
    role: "tool",
    title,
    output,
    toolCallId: title,
    status,
    kind: "other",
  };
}

describe("quiet lines", () => {
  it("reads a settled agent row as one line: name, state, and the compact duration", async () => {
    await show([
      {
        id: "sys-1",
        role: "system",
        severity: "info",
        text: "Background agent finished · Explore auth · took 7m 0s",
      },
    ]);

    expect(host.textContent).toContain("Explore auth finished · 7m");
    expect(host.textContent).not.toContain("Background agent");
    expect(host.querySelector(".workspace-chat-quiet-line svg")).toBeNull();
  });

  it("marks a failed agent row with the triangle, and keeps the word in the line", async () => {
    await show([
      {
        id: "sys-1",
        role: "system",
        severity: "info",
        text: "Background agent failed · Explore auth · took 3m 0s",
      },
    ]);

    expect(host.textContent).toContain("Explore auth failed · 3m");
    expect(host.querySelector(".workspace-chat-quiet-line svg")).not.toBeNull();
  });

  it("opens a turn error's detail only on click", async () => {
    await show([
      { id: "err-1", role: "error", text: "402 Payment Required", detail: "raw provider body" },
    ]);

    const details = host.querySelector("details");
    expect(details).not.toBeNull();
    expect(details?.open).toBe(false);
    expect(host.querySelector("summary")?.textContent).toBe("402 Payment Required");
    expect(details?.querySelector(".workspace-chat-error-detail")?.textContent).toBe(
      "raw provider body",
    );
  });

  it("reads a created agent as a start line with no output behind it", async () => {
    await show([
      call(
        "mcp__devboule__devboule_create_agent",
        "completed",
        '{"displayName":"Website redesign","sessionId":"s.1"}',
      ),
    ]);

    expect(host.textContent).toContain("Started agent · Website redesign");
    expect(host.textContent).not.toContain("sessionId");
    expect(host.querySelector("details")).toBeNull();
  });

  it("reads a failed create as a sentence with no detail, and never shows its raw result", async () => {
    await show([
      call(
        "mcp__devboule__devboule_create_agent",
        "failed",
        "Mcp error -32602: provider is required",
      ),
    ]);

    expect(host.textContent).toContain("Couldn't start the agent");
    expect(host.textContent).not.toContain("Mcp error");
    expect(host.querySelector("details")).toBeNull();
  });

  it("names an unrecovered failed command with its failure word and triangle", async () => {
    await show([
      {
        id: "t-1",
        role: "tool",
        title: "npm test",
        output: "",
        toolCallId: "t-1",
        status: "failed",
        kind: "execute",
      },
    ]);

    expect(host.querySelector(".workspace-chat-tool-failed")?.textContent).toBe("failed");
    expect(host.querySelector(".workspace-chat-tool-summary svg")).not.toBeNull();
  });

  it("humanises a Devboule tool's raw name instead of drawing it", async () => {
    await show([call("mcp__devboule__devboule_archive_workspace", "completed")]);

    expect(host.textContent).toContain("Archived workspace");
    expect(host.textContent).not.toContain("mcp__devboule__");
  });
});
