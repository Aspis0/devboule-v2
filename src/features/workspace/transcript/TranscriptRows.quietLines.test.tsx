// @vitest-environment happy-dom

// Each row kind reads as one quiet line; its detail waits behind the click.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { useAppStore } from "../../../store/appStore";
import type { DesignHost, DesignMessage } from "../../design/designHost";
import { delegatedDesignCardId } from "../../design/delegatedDesignMirror";
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

    expect(host.querySelector(".workspace-chat-tool-failed")?.textContent).toBe("· failed");
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

const ROW_TEST_DOCUMENT = {
  name: "",
  path: "",
  contextPrefix: "Editing",
  draftPlaceholder: "Describe the change…",
  noContextPlaceholder: "Describe what to generate…",
  initialState: { zoom: 1, saved: false, draft: "", hiddenLayerIds: [] },
  selectedLayerId: "",
  grounded: true,
  layers: [],
  messages: [],
  workingMessage: { title: "Generating…", desc: "Asking the agent." },
};

const ROW_HOST: DesignHost = {
  loadDocument: async () => ({ ...ROW_TEST_DOCUMENT }),
};

function rowMirroredCard(childSessionId: string, html: string): DesignMessage {
  return {
    id: delegatedDesignCardId(childSessionId),
    role: "assistant",
    status: "done",
    title: childSessionId,
    desc: "Finished by a Design agent.",
    sources: [],
    nodeIds: [],
    artifactHtml: html,
  } as DesignMessage;
}

function setRowCanvas(messages: DesignMessage[]): void {
  useAppStore.setState({
    activeSurface: "workspace",
    designSession: {
      host: ROW_HOST,
      document: null,
      messages: [],
      latestArtifact: null,
      generation: null,
      sectionNotes: [],
      lastCanvasPageHeight: null,
      mirroredFitPending: false,
    },
  });
  useAppStore.getState().setDesignDocument(ROW_HOST, { ...ROW_TEST_DOCUMENT }, messages);
}

function clearRowCanvas(): void {
  useAppStore.setState({
    activeSurface: "workspace",
    designSession: {
      host: null,
      document: null,
      messages: [],
      latestArtifact: null,
      generation: null,
      sectionNotes: [],
      lastCanvasPageHeight: null,
      mirroredFitPending: false,
    },
  });
}

function finishedAgentRow(childSessionId: string): AgentChatItem {
  return {
    id: "sys-design-row",
    role: "system",
    severity: "info",
    text: "Background agent finished · Mockup bar italiano · took 24m 58s",
    task: {
      kind: "agent",
      state: "finished",
      title: "Mockup bar italiano",
      childSessionId,
      durationMs: 1_498_000,
    },
  };
}

async function showRow(items: AgentChatItem[], onOpenChild?: (sessionId: string) => void) {
  await act(async () =>
    root?.render(
      <TranscriptRows
        items={items}
        recoveredAttach={false}
        a2aNames={names}
        fileLinks={null}
        transcriptEnded={false}
        streamingThoughtId={null}
        onOpenChild={onOpenChild}
      />,
    ),
  );
}

describe("finished task row design action", () => {
  beforeEach(() => {
    clearRowCanvas();
  });

  afterEach(() => {
    clearRowCanvas();
  });

  it("shows the open action beside the finished row when the mirrored card exists", async () => {
    setRowCanvas([rowMirroredCard("child-a", "<main>A</main>")]);
    await showRow([finishedAgentRow("child-a")], vi.fn());

    const action = host.querySelector<HTMLButtonElement>(".workspace-chat-activity-action");
    expect(action?.textContent).toBe("Open in Design");
    expect(host.textContent).toContain("Mockup bar italiano finished");
  });

  it("shows no action without a card and shows it when the card lands after mount", async () => {
    setRowCanvas([]);
    await showRow([finishedAgentRow("child-c")], vi.fn());
    expect(host.querySelector(".workspace-chat-activity-action")).toBeNull();

    await act(async () => {
      useAppStore
        .getState()
        .setDesignMessages(ROW_HOST, [rowMirroredCard("child-c", "<main>C</main>")]);
    });

    expect(
      host.querySelector<HTMLButtonElement>(".workspace-chat-activity-action")?.textContent,
    ).toBe("Open in Design");
  });

  it("keeps the action out of the row button", async () => {
    setRowCanvas([rowMirroredCard("child-a", "<main>A</main>")]);
    await showRow([finishedAgentRow("child-a")], vi.fn());

    expect(host.querySelector("button button")).toBeNull();
    const buttons = Array.from(host.querySelectorAll<HTMLButtonElement>("button"));
    expect(buttons).toHaveLength(2);
  });

  it("opens the child from the row and the mirrored artifact from the action", async () => {
    setRowCanvas([
      rowMirroredCard("child-a", "<main>A</main>"),
      rowMirroredCard("child-b", "<main>B</main>"),
    ]);
    const opened = vi.fn();
    await showRow([finishedAgentRow("child-a")], opened);

    const buttons = Array.from(host.querySelectorAll<HTMLButtonElement>("button"));
    expect(buttons).toHaveLength(2);
    await act(async () => {
      buttons[0]?.click();
    });
    expect(opened).toHaveBeenCalledWith("child-a");
    expect(useAppStore.getState().activeSurface).toBe("workspace");

    await act(async () => {
      buttons[1]?.click();
    });
    expect(useAppStore.getState().activeSurface).toBe("design");
    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>A</main>",
    });
  });

  it("shows no action for a failed row even when its card exists", async () => {
    setRowCanvas([rowMirroredCard("child-a", "<main>A</main>")]);
    await showRow([
      {
        id: "sys-failed",
        role: "system",
        severity: "info",
        text: "Background agent failed · Mockup bar italiano · took 3m 0s",
        task: {
          kind: "agent",
          state: "failed",
          title: "Mockup bar italiano",
          childSessionId: "child-a",
          durationMs: 180_000,
        },
      },
    ]);

    expect(host.querySelector(".workspace-chat-activity-action")).toBeNull();
  });

  it("shows no action for a command row", async () => {
    setRowCanvas([rowMirroredCard("child-a", "<main>A</main>")]);
    await showRow([
      {
        id: "sys-command",
        role: "system",
        severity: "info",
        text: "Background command finished · npm test · took 4s",
        task: { kind: "command", state: "finished", title: "npm test", durationMs: 4_000 },
      },
    ]);

    expect(host.querySelector(".workspace-chat-activity-action")).toBeNull();
  });
});
