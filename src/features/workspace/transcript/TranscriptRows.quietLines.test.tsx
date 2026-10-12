// @vitest-environment happy-dom

// Each row kind reads as one quiet line; its detail waits behind the click.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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
        task: { kind: "agent", state: "finished", title: "Explore auth", durationMs: 420_000 },
      },
    ]);

    expect(host.textContent).toContain("Explore auth finished · 7m");
    expect(host.textContent).not.toContain("Background agent");
    expect(host.querySelector(".workspace-chat-quiet-line svg")).toBeNull();
  });

  it("draws a background command's row as one quiet line, in its own wording", async () => {
    await show([
      {
        id: "sys-1",
        role: "system",
        severity: "info",
        text: "Background command finished · npm test · took 4s",
        task: { kind: "command", state: "finished", title: "npm test", durationMs: 4_000 },
      },
    ]);

    expect(host.querySelector(".workspace-chat-quiet-line")?.textContent).toBe(
      "Background command finished · npm test · took 4s",
    );
  });

  it("marks a failed agent row with the triangle, and keeps the word in the line", async () => {
    await show([
      {
        id: "sys-1",
        role: "system",
        severity: "info",
        text: "Background agent failed · Explore auth · took 3m 0s",
        task: { kind: "agent", state: "failed", title: "Explore auth", durationMs: 180_000 },
      },
    ]);

    expect(host.textContent).toContain("Explore auth failed · 3m");
    expect(host.querySelector(".workspace-chat-quiet-line svg")).not.toBeNull();
  });

  it("opens a finished child's tab from its line, and does not draw the raw row text", async () => {
    const opened = vi.fn();
    root?.unmount();
    root = createRoot(host);
    await act(async () =>
      root?.render(
        <TranscriptRows
          items={[
            {
              id: "sys-1",
              role: "system",
              severity: "info",
              text: "Background agent finished · Explore auth · took 7m 0s",
              task: {
                kind: "agent",
                state: "finished",
                title: "Explore auth",
                childSessionId: "s.kid.1",
                durationMs: 420_000,
              },
            },
          ]}
          recoveredAttach={false}
          a2aNames={names}
          fileLinks={null}
          transcriptEnded={false}
          streamingThoughtId={null}
          onOpenChild={opened}
        />,
      ),
    );

    await act(async () => host.querySelector<HTMLButtonElement>("button")?.click());
    expect(opened).toHaveBeenCalledWith("s.kid.1");
  });

  it("opens the child a started agent's result names, without drawing that result", async () => {
    const opened = vi.fn();
    root?.unmount();
    root = createRoot(host);
    await act(async () =>
      root?.render(
        <TranscriptRows
          items={[
            call(
              "mcp__devboule__devboule_create_agent",
              "completed",
              '{"displayName":"Website redesign","sessionId":"s.site.1"}',
            ),
          ]}
          recoveredAttach={false}
          a2aNames={names}
          fileLinks={null}
          transcriptEnded={false}
          streamingThoughtId={null}
          onOpenChild={opened}
        />,
      ),
    );

    await act(async () => host.querySelector<HTMLButtonElement>("button")?.click());
    expect(opened).toHaveBeenCalledWith("s.site.1");
    expect(host.textContent).not.toContain("sessionId");
  });

  it("opens a turn error's detail only on click", async () => {
    await show([
      { id: "err-1", role: "error", text: "402 Payment Required", detail: "raw provider body" },
    ]);

    const details = host.querySelector("details");
    expect(details).not.toBeNull();
    expect(details?.open).toBe(false);
    expect(host.querySelector("summary")?.textContent).toBe("402 Payment Required");
    expect(host.querySelector(".workspace-chat-error-detail")).toBeNull();
    await act(async () => host.querySelector("summary")?.click());
    expect(host.querySelector(".workspace-chat-error-detail")?.textContent).toBe(
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

  it("reads a failed create as its sentence with the reason on the line, and opens to the error", async () => {
    await show([
      call(
        "mcp__devboule__devboule_create_agent",
        "failed",
        "Mcp error -32602: provider is required",
      ),
    ]);

    expect(host.textContent).toContain("Couldn't start the agent");
    expect(host.textContent).toContain("provider is required");
    expect(host.textContent).not.toContain("Mcp error");
    expect(host.querySelector("details")).not.toBeNull();
    await act(async () => host.querySelector("summary")?.click());
    expect(host.textContent).toContain("Mcp error -32602: provider is required");
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

  it("humanises a provider's devboule name that has no devboule_ prefix", async () => {
    await show([call("mcp__devboule__list_workspaces", "completed")]);

    expect(host.textContent).toContain("Listed workspaces");
    expect(host.textContent).not.toContain("mcp__devboule__");
  });

  it("shows a creation that did not start as a failure, even when its call completed", async () => {
    await show([
      call(
        "mcp__devboule__devboule_create_agent",
        "completed",
        '{"displayName":"Kid","sessionId":"s.k.1"}',
      ),
      {
        id: "sys-1",
        role: "system",
        severity: "info",
        text: "The agent creation did not start: the provider is required.",
      },
    ]);

    expect(host.textContent).toContain("Couldn't start the agent");
    expect(host.textContent).not.toContain("did not start");
    // The same creation reads once: its start line does not stand beside the failure.
    expect(host.textContent).not.toContain("Started agent");
  });

  it("humanises a Devboule tool's raw name instead of drawing it", async () => {
    await show([call("mcp__devboule__devboule_archive_workspace", "completed")]);

    expect(host.textContent).toContain("Archived workspace");
    expect(host.textContent).not.toContain("mcp__devboule__");
  });
});
