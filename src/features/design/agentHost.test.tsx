// @vitest-environment happy-dom

import { StrictMode, act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  OracleIndexStatus,
  OracleSearchResponse,
  Session,
  SessionEvent,
  Workspace,
} from "../../types/ipc";

const channelHarness = vi.hoisted(() => ({
  emit: null as ((event: SessionEvent) => void) | null,
  active: null as ((event: SessionEvent) => void) | null,
  handlers: new WeakMap<object, (event: SessionEvent) => void>(),
}));

const mocks = vi.hoisted(() => ({
  oracleAsk: vi.fn(),
  oracleFiles: vi.fn(),
  oracleStatus: vi.fn(),
  reasonFromCause: vi.fn(),
  projectsList: vi.fn(),
  workspacesList: vi.fn(),
  sessionCreate: vi.fn(),
  sessionAttach: vi.fn(),
  sessionSend: vi.fn(),
  sessionInterrupt: vi.fn(),
  sessionDetach: vi.fn(),
  sessionClose: vi.fn(),
  sessionPermissionRespond: vi.fn(),
  pluginsList: vi.fn(),
}));

vi.mock("../../lib/tauri", () => ({
  createSessionChannel: vi.fn((onEvent: (event: SessionEvent) => void) => {
    const channel = {};
    channelHarness.handlers.set(channel, onEvent);
    channelHarness.emit = onEvent;
    return channel;
  }),
  oracleAsk: mocks.oracleAsk,
  oracleFiles: mocks.oracleFiles,
  oracleStatus: mocks.oracleStatus,
  reasonFromCause: mocks.reasonFromCause,
  projectsList: mocks.projectsList,
  workspacesList: mocks.workspacesList,
  sessionCreate: mocks.sessionCreate,
  sessionAttach: mocks.sessionAttach,
  sessionSend: mocks.sessionSend,
  sessionInterrupt: mocks.sessionInterrupt,
  sessionDetach: mocks.sessionDetach,
  sessionClose: mocks.sessionClose,
  sessionPermissionRespond: mocks.sessionPermissionRespond,
  pluginsList: mocks.pluginsList,
}));

import { App } from "../../app/App";
import { useAppStore } from "../../store/appStore";
import type { AgentSessionState } from "../../lib/agentSession";
import type { DesignGenerationResult } from "./designHost";
import { builtInSkillIndex, builtInSkillSources } from "./builtInSkills";
import { parseSkillFile } from "./skillLoader";
import {
  DESIGN_DOCTRINE_BEGIN,
  DESIGN_DOCTRINE_END,
  DESIGN_DOCTRINE_RESTATEMENT,
  createAgentHost,
  disposeAgentHost,
  extractArtifactHtml,
  extractFencedHtml,
  groundedPrompt,
  MAX_ARTIFACT_BYTES,
} from "./agentHost";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const PROJECT = { id: "project-1", name: "Devboule", path: "C:/devboule" };
const WORKSPACE: Workspace = {
  id: "workspace-1",
  projectId: PROJECT.id,
  title: "feat/design",
  isolation: "worktree",
};
const SESSION: Session = {
  id: "session-1",
  workspaceId: WORKSPACE.id,
  kind: "acp",
  title: "Design agent",
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
};
const READY_STATUS = {
  state: "ready",
  indexed_files: 1,
  model: { state: "ready" },
  reranker: null,
} as OracleIndexStatus;

async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

function createRootContainer(): { container: HTMLDivElement; root: Root } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  return { container, root: createRoot(container) };
}

function finishRun(): void {
  if (channelHarness.active === null) throw new Error("session channel was not active at finish");
  channelHarness.active({ type: "agent_finished", stopReason: "end_turn" });
}

function emitToolCall(
  toolCallId: string,
  status: string,
  kind?: string,
  paths?: readonly string[],
): void {
  channelHarness.active?.({
    type: "agent_tool_call",
    toolCallId,
    title: "Tool",
    status,
    ...(kind === undefined ? {} : { kind }),
    ...(paths === undefined ? {} : { locations: paths.map((path) => ({ path })) }),
  });
}

function emitToolUpdate(
  toolCallId: string,
  status: string | null,
  text: string | null,
  paths?: readonly string[],
): void {
  channelHarness.active?.({
    type: "agent_tool_update",
    toolCallId,
    status,
    text,
    ...(paths === undefined ? {} : { locations: paths.map((path) => ({ path })) }),
  });
}

async function startRun(
  host: ReturnType<typeof createAgentHost>,
  options?: { skills?: readonly string[] },
): Promise<{ run: Promise<DesignGenerationResult> }> {
  const run = host.generate?.("Update the design", new AbortController().signal, options);
  let failure: unknown;
  void run?.catch((error: unknown) => {
    failure = error;
  });
  for (let index = 0; index < 12; index += 1) await Promise.resolve();
  if (failure !== undefined) {
    throw new Error(`generation failed: ${failure instanceof Error ? failure.message : failure}`);
  }
  if (!mocks.sessionSend.mock.calls.length) throw new Error("session send did not start");
  if (channelHarness.active === null) throw new Error("session channel was not active");
  return { run: run as Promise<DesignGenerationResult> };
}

beforeEach(() => {
  useAppStore.setState({
    activeSurface: "design",
    plugins: null,
    installing: null,
    installError: null,
  });
  channelHarness.emit = null;
  channelHarness.active = null;
  mocks.oracleAsk.mockReset();
  mocks.oracleFiles.mockReset();
  mocks.oracleStatus.mockReset();
  mocks.reasonFromCause.mockReset();
  mocks.projectsList.mockReset();
  mocks.workspacesList.mockReset();
  mocks.sessionCreate.mockReset();
  mocks.sessionAttach.mockReset();
  mocks.sessionSend.mockReset();
  mocks.sessionInterrupt.mockReset();
  mocks.sessionDetach.mockReset();
  mocks.sessionClose.mockReset();
  mocks.sessionPermissionRespond.mockReset();
  mocks.pluginsList.mockReset();

  mocks.oracleAsk.mockResolvedValue({
    query: "Update the design",
    results: [
      {
        path: "src/app/Shell.tsx",
        line_start: 1,
        line_end: 4,
        snippet: "export function Shell() {}",
        score: 0.9,
      },
    ],
  });
  mocks.oracleFiles.mockResolvedValue([]);
  mocks.reasonFromCause.mockImplementation((cause: unknown) =>
    cause instanceof Error ? cause.message : String(cause),
  );
  mocks.projectsList.mockResolvedValue([PROJECT]);
  mocks.workspacesList.mockResolvedValue([WORKSPACE]);
  mocks.sessionCreate.mockResolvedValue(SESSION);
  mocks.sessionAttach.mockImplementation(async (...args: unknown[]) => {
    const channel = args[2];
    channelHarness.active =
      typeof channel === "object" && channel !== null
        ? (channelHarness.handlers.get(channel) ?? null)
        : null;
  });
  mocks.sessionSend.mockResolvedValue(undefined);
  mocks.sessionInterrupt.mockResolvedValue(undefined);
  mocks.sessionDetach.mockResolvedValue(undefined);
  mocks.sessionClose.mockResolvedValue(undefined);
  mocks.pluginsList.mockResolvedValue({ root: "", plugins: [], problem: null });
});

afterEach(() => {
  document.body.replaceChildren();
});

describe("ACP design host", () => {
  it("sends only the explicitly selected doctrine section", async () => {
    const index = builtInSkillIndex();
    const selected = index[0];
    const omitted = index[1];
    if (selected === undefined || omitted === undefined) throw new Error("Built-in skills missing");

    const host = createAgentHost();
    const { run } = await startRun(host, { skills: [selected.slug] });
    finishRun();
    await run;

    const sentText = mocks.sessionSend.mock.calls[0]?.[1] as string;
    expect(sentText).toContain(`## ${selected.title}`);
    const selectedSource = builtInSkillSources().find((source) =>
      source.path.endsWith(`${selected.slug}.md`),
    );
    if (selectedSource === undefined) throw new Error("Selected skill source missing");
    const selectedSection = parseSkillFile(selectedSource.path, selectedSource.text);
    if (!selectedSection.ok) throw new Error("Selected skill did not parse");
    expect(sentText).toContain(selectedSection.section.body);
    expect(sentText).not.toContain(`## ${omitted.title}`);

    await disposeAgentHost(host);
  });

  it("omits the doctrine block when an empty skill list is selected", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host, { skills: [] });
    finishRun();
    await run;

    const sentText = mocks.sessionSend.mock.calls[0]?.[1] as string;
    expect(sentText).not.toContain(DESIGN_DOCTRINE_BEGIN);
    expect(sentText).not.toContain(DESIGN_DOCTRINE_END);

    await disposeAgentHost(host);
  });

  it("sends every built-in doctrine section when all are selected", async () => {
    const index = builtInSkillIndex();
    const host = createAgentHost();
    const { run } = await startRun(host, { skills: index.map((entry) => entry.slug) });
    finishRun();
    await run;

    const sentText = mocks.sessionSend.mock.calls[0]?.[1] as string;
    for (const entry of index) expect(sentText).toContain(`## ${entry.title}`);

    await disposeAgentHost(host);
  });

  it("reports paths from a completed write tool as sources", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);

    emitToolCall("write-1", "completed", "edit", ["src/features/design/DesignSurface.tsx"]);
    finishRun();

    const result = await run;
    expect(result.sources).toEqual(["src/features/design/DesignSurface.tsx"]);
    expect(result.desc).toContain("wrote 1 file");
    expect(result.desc).toContain("DesignSurface.tsx");
    expect(result.title).toContain("wrote");
    expect(result.desc).toContain("Review what the agent wrote with your own git.");
    expect(mocks.sessionCreate).toHaveBeenCalledWith(WORKSPACE.id, "acp");
    expect(mocks.sessionAttach).toHaveBeenCalledWith("session-1", null, expect.anything());
    expect(mocks.sessionSend).toHaveBeenCalledWith(
      "session-1",
      expect.stringContaining("src/app/Shell.tsx:1-4"),
    );

    await disposeAgentHost(host);
  });

  it("replaces a tool's earlier locations when an update reports new ones", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);

    emitToolCall("write-1", "in_progress", "edit", ["src/old.ts"]);
    emitToolUpdate("write-1", "completed", "Finished", ["src/new.ts"]);
    finishRun();

    const result = await run;
    expect(result.sources).toEqual(["src/new.ts"]);
    expect(result.sources).not.toContain("src/old.ts");

    await disposeAgentHost(host);
  });

  it("ignores locations from non-write tool kinds", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);

    emitToolCall("read-1", "completed", "read", ["src/read-only.ts"]);
    finishRun();

    const result = await run;
    expect(result.sources).toEqual([]);
    expect(result.desc).toContain("No files were reported as written");

    await disposeAgentHost(host);
  });

  it("does not count a write until its tool completes", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);

    emitToolCall("write-1", "in_progress", "edit", ["src/pending.ts"]);
    finishRun();

    const result = await run;
    expect(result.sources).toEqual([]);
    expect(result.desc).toContain("No files were reported as written");

    await disposeAgentHost(host);
  });

  it("keeps a completed write counted after a later status update", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);

    emitToolCall("write-1", "completed", "edit", ["src/done.ts"]);
    emitToolUpdate("write-1", "in_progress", "A later update arrived.");
    finishRun();

    const result = await run;
    expect(result.sources).toEqual(["src/done.ts"]);

    await disposeAgentHost(host);
  });

  it("warns that completed shell commands may hide additional changes", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);

    emitToolCall("shell-1", "completed", "execute");
    finishRun();

    const result = await run;
    expect(result.desc).toContain("shell commands");
    expect(result.desc).toContain("may also have changed");

    await disposeAgentHost(host);
  });

  it("does not report a failed write as a written file", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);
    emitToolCall("write-1", "failed", "edit", ["src/failed.ts"]);
    finishRun();

    const result = await run;
    expect(result.sources).not.toContain("src/failed.ts");
    expect(result.title).toBe("Agent wrote no files");

    await disposeAgentHost(host);
  });

  it("says when the agent does not report locations", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);
    finishRun();

    const result = await run;
    expect(result.sources).toEqual([]);
    expect(result.desc).toContain("did not report which files it touched");

    await disposeAgentHost(host);
  });

  it("fails honestly when no workspace is available", async () => {
    mocks.projectsList.mockResolvedValue([]);
    const host = createAgentHost();

    await expect(
      host.generate?.("Update the design", new AbortController().signal),
    ).rejects.toThrow("No workspace is available");
    expect(mocks.oracleAsk).toHaveBeenCalledWith("Update the design");
    expect(mocks.sessionCreate).not.toHaveBeenCalled();
  });

  it("rejects when AgentSession cannot attach", async () => {
    mocks.sessionAttach.mockRejectedValue(new Error("attach failed"));
    const host = createAgentHost();

    await expect(
      host.generate?.("Update the design", new AbortController().signal),
    ).rejects.toThrow("Could not attach the agent session: attach failed");
    expect(mocks.sessionSend).not.toHaveBeenCalled();

    await disposeAgentHost(host);
  });

  it("rejects when AgentSession cannot send", async () => {
    mocks.sessionSend.mockRejectedValue(new Error("send failed"));
    const host = createAgentHost();

    await expect(
      host.generate?.("Update the design", new AbortController().signal),
    ).rejects.toThrow("Could not send the message: send failed");

    await disposeAgentHost(host);
  });

  it("uses AgentSession's error when the agent exits during a turn", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);

    channelHarness.active?.({ type: "exit", code: 1 });

    await expect(run).rejects.toThrow("The agent stopped before finishing this turn.");
    await disposeAgentHost(host);
  });

  it("stops on a permission request without approving it", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);

    channelHarness.active?.({
      type: "permission_request",
      toolCallId: "permission-1",
      title: "Write a file",
      command: "apply_patch",
      options: [{ optionId: "allow", name: "Allow once", kind: "allow_once" }],
    });

    await expect(run).rejects.toThrow("Respond in the Workspace surface");
    expect(mocks.sessionPermissionRespond).not.toHaveBeenCalled();
    expect(mocks.sessionInterrupt).toHaveBeenCalledWith("session-1");

    await disposeAgentHost(host);
  });

  it("interrupts an active run when its signal is aborted", async () => {
    const host = createAgentHost();
    const controller = new AbortController();
    const run = host.generate?.("Update the design", controller.signal);
    await vi.waitFor(() => expect(mocks.sessionSend).toHaveBeenCalled());

    controller.abort();

    await expect(run).rejects.toMatchObject({ name: "AbortError" });
    expect(mocks.sessionInterrupt).toHaveBeenCalledWith("session-1");
    await disposeAgentHost(host);
  });

  it("reuses one ACP session across generations", async () => {
    const host = createAgentHost();
    const { run: first } = await startRun(host);
    finishRun();
    await first;

    const { run: second } = await startRun(host);
    finishRun();
    await second;

    expect(mocks.sessionCreate).toHaveBeenCalledTimes(1);
    expect(mocks.sessionAttach).toHaveBeenCalledTimes(1);
    expect(mocks.sessionSend).toHaveBeenCalledTimes(2);
    await disposeAgentHost(host);
  });

  it("extracts an artifact only from the current generation turn", async () => {
    const host = createAgentHost();
    const { run: first } = await startRun(host);
    channelHarness.active?.({
      type: "agent_message",
      messageId: "m-1",
      text: "```html\n<div>First artifact</div>\n```",
    });
    finishRun();
    expect((await first).artifactHtml).toBe("<div>First artifact</div>");

    const { run: second } = await startRun(host);
    channelHarness.active?.({
      type: "agent_message",
      messageId: "m-2",
      text: "This turn has no artifact.",
    });
    finishRun();

    expect((await second).artifactHtml).toBeUndefined();
    await disposeAgentHost(host);
  });

  it("reopens a session that closed between generations", async () => {
    const host = createAgentHost();
    const { run: first } = await startRun(host);
    finishRun();
    await first;

    channelHarness.active?.({ type: "exit", code: 1 });

    const { run: second } = await startRun(host);
    finishRun();
    await second;

    expect(mocks.sessionCreate).toHaveBeenCalledTimes(2);
    expect(mocks.sessionAttach).toHaveBeenCalledTimes(2);
    expect(mocks.sessionClose).toHaveBeenCalledWith("session-1");
    await disposeAgentHost(host);
  });

  it("rejects an overlapping generation without disturbing the first", async () => {
    const host = createAgentHost();
    const { run: first } = await startRun(host);

    const second = host.generate?.("Another design", new AbortController().signal);
    await expect(second).rejects.toThrow("A design generation is already running.");

    finishRun();
    await first;
    await disposeAgentHost(host);
  });

  it("lets abort reject while sending is still pending", async () => {
    mocks.sessionSend.mockImplementation(() => new Promise<void>(() => undefined));
    const host = createAgentHost();
    const controller = new AbortController();
    const run = host.generate?.("Update the design", controller.signal);
    await vi.waitFor(() => expect(mocks.sessionSend).toHaveBeenCalled());

    controller.abort();
    const outcome = await Promise.race([
      run?.then(
        () => "resolved",
        (error: unknown) => error,
      ),
      new Promise<"timed out">((resolve) => setTimeout(() => resolve("timed out"), 100)),
    ]);
    expect(outcome).toMatchObject({ name: "AbortError" });

    await disposeAgentHost(host);
  });

  it("does not create a session when disposed during Oracle grounding", async () => {
    let resolveOracle: ((response: OracleSearchResponse) => void) | undefined;
    mocks.oracleAsk.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveOracle = resolve;
        }),
    );
    const host = createAgentHost();
    const run = host.generate?.("Update the design", new AbortController().signal);

    await disposeAgentHost(host);
    resolveOracle?.({ query: "Update the design", results: [] });

    await expect(run).rejects.toThrow("The design surface is no longer available.");
    expect(mocks.sessionCreate).not.toHaveBeenCalled();
  });

  it("closes and detaches the agent session when the Design mount unmounts", async () => {
    mocks.oracleStatus.mockResolvedValue(READY_STATUS);
    const { container, root } = createRootContainer();

    await act(async () => root.render(<App />));
    await act(async () => undefined);
    await act(async () => undefined);
    await vi.waitFor(() =>
      expect(
        container.querySelector<HTMLTextAreaElement>(
          'textarea[aria-label="Describe a design change"]',
        ),
      ).not.toBeNull(),
    );
    expect(container.textContent).toContain(
      "Repository agent — ACP writes in the first workspace it finds.",
    );

    const draft = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Describe a design change"]',
    );
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (draft === null || send === null) throw new Error("Design composer did not render");
    const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("textarea value setter did not exist");
    setValue.call(draft, "Update the design");
    draft.dispatchEvent(new Event("input", { bubbles: true }));
    await act(async () => send.click());
    await settle();

    await act(async () => root.unmount());
    await settle();

    expect(mocks.sessionDetach).toHaveBeenCalledWith("session-1");
    expect(mocks.sessionClose).toHaveBeenCalledWith("session-1");
  });

  it("creates a fresh agent host after StrictMode effect cleanup", async () => {
    mocks.oracleStatus.mockResolvedValue(READY_STATUS);
    const { container, root } = createRootContainer();

    await act(async () =>
      root.render(
        <StrictMode>
          <App />
        </StrictMode>,
      ),
    );
    await act(async () => undefined);
    await act(async () => undefined);
    await vi.waitFor(() =>
      expect(
        container.querySelector<HTMLTextAreaElement>(
          'textarea[aria-label="Describe a design change"]',
        ),
      ).not.toBeNull(),
    );

    const draft = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Describe a design change"]',
    );
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (draft === null || send === null) throw new Error("Design composer did not render");
    const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("textarea value setter did not exist");
    setValue.call(draft, "Update the design");
    draft.dispatchEvent(new Event("input", { bubbles: true }));
    await act(async () => send.click());

    await vi.waitFor(() => expect(mocks.sessionSend).toHaveBeenCalled());
    await act(async () => finishRun());
    await settle();

    await act(async () => root.unmount());
    await settle();
  });

  describe("artifact HTML extraction", () => {
    describe("extractFencedHtml", () => {
      it("returns undefined when there is no fenced block", () => {
        expect(extractFencedHtml("Just plain text with no fence.")).toBeUndefined();
      });

      it("returns undefined for an empty or whitespace-only block", () => {
        expect(extractFencedHtml("text\n```html\n   \n```\nmore")).toBeUndefined();
        expect(extractFencedHtml("text\n```html\n\n```\nmore")).toBeUndefined();
      });

      it("returns undefined when the fence opens but never closes", () => {
        expect(extractFencedHtml("prefix\n```html\n<div>content</div>")).toBeUndefined();
      });

      it("returns the last non-empty block when multiple are present", () => {
        const text = [
          "First block:",
          "```html",
          "<div>First</div>",
          "```",
          "",
          "Second block:",
          "```html",
          "<span>Second</span>",
          "```",
        ].join("\n");
        expect(extractFencedHtml(text)).toBe("<span>Second</span>");
      });

      it("does not strip </iframe> from the block content", () => {
        const text = "text\n```html\n<div></iframe></div>\n```";
        expect(extractFencedHtml(text)).toBe("<div></iframe></div>");
      });

      it("does not strip <script> from the block content", () => {
        const text = "text\n```html\n<script>alert(1)</script>\n```";
        expect(extractFencedHtml(text)).toBe("<script>alert(1)</script>");
      });

      it("accepts a block without a trailing newline before the closing fence", () => {
        expect(extractFencedHtml("```html\n<div>tight</div>\n```")).toBe("<div>tight</div>");
      });

      it("accepts a block without a leading newline after the opening fence", () => {
        expect(extractFencedHtml("```html\n<main>Hi</main>\n```")).toBe("<main>Hi</main>");
      });

      it("accepts a single-line block with both fences on the same line as content", () => {
        const text = "prefix\n```html\n<p>Hello</p>\n```\nsuffix";
        expect(extractFencedHtml(text)).toBe("<p>Hello</p>");
      });
    });

    describe("extractArtifactHtml", () => {
      it("scans assistant messages only, not thoughts", () => {
        const state: AgentSessionState = {
          items: [
            {
              id: "t-1",
              role: "thought",
              text: "I could write ```html\n<div>Musing</div>\n```",
              messageId: null,
            },
            {
              id: "a-1",
              role: "assistant",
              text: "Here is the result:\n```html\n<div>Final</div>\n```",
              messageId: "m-1",
            },
          ],
          status: "idle",
          streaming: false,
          availableCommands: [],
          lastFinished: null,
          manifest: null,
          pendingSwitch: null,
        };
        expect(extractArtifactHtml(state)).toBe("<div>Final</div>");
      });

      it("returns undefined when no assistant message has a block", () => {
        const state: AgentSessionState = {
          items: [
            {
              id: "a-1",
              role: "assistant",
              text: "No fence here.",
              messageId: "m-1",
            },
          ],
          status: "idle",
          streaming: false,
          availableCommands: [],
          lastFinished: null,
          manifest: null,
          pendingSwitch: null,
        };
        expect(extractArtifactHtml(state)).toBeUndefined();
      });

      it("returns undefined when a thought has a block but the assistant does not", () => {
        const state: AgentSessionState = {
          items: [
            {
              id: "t-1",
              role: "thought",
              text: "I could write ```html\n<div>Musing</div>\n```",
              messageId: null,
            },
            {
              id: "a-1",
              role: "assistant",
              text: "Here is my answer without a fence.",
              messageId: "m-1",
            },
          ],
          status: "idle",
          streaming: false,
          availableCommands: [],
          lastFinished: null,
          manifest: null,
          pendingSwitch: null,
        };
        expect(extractArtifactHtml(state)).toBeUndefined();
      });

      it("returns the latest block from the last assistant message", () => {
        const state: AgentSessionState = {
          items: [
            {
              id: "a-1",
              role: "assistant",
              text: "First: ```html\n<div>First</div>\n```",
              messageId: "m-1",
            },
            {
              id: "a-2",
              role: "assistant",
              text: "Second: ```html\n<div>Second</div>\n```",
              messageId: "m-2",
            },
          ],
          status: "idle",
          streaming: false,
          availableCommands: [],
          lastFinished: null,
          manifest: null,
          pendingSwitch: null,
        };
        expect(extractArtifactHtml(state)).toBe("<div>Second</div>");
      });
    });

    describe("grounding prompt content", () => {
      it("does not mention Workspace Changes or a Changes panel", async () => {
        const host = createAgentHost();
        const { run } = await startRun(host);
        finishRun();
        await run;

        const sentText = mocks.sessionSend.mock.calls[0]?.[1] as string;
        expect(sentText).not.toContain("Workspace Changes");
        expect(sentText).not.toContain("Changes panel");
        expect(sentText).not.toContain("authoritative");

        await disposeAgentHost(host);
      });

      it("carries a hit's line range into the prompt", async () => {
        const host = createAgentHost();
        const { run } = await startRun(host);
        finishRun();
        await run;

        const sentText = mocks.sessionSend.mock.calls[0]?.[1] as string;
        expect(sentText).toContain("src/app/Shell.tsx:1-4");

        await disposeAgentHost(host);
      });

      it("does not print empty brackets when a hit has no symbol name", async () => {
        mocks.oracleAsk.mockResolvedValueOnce({
          query: "Find the workspace resolver.",
          results: [
            {
              path: "src/lib/workspace.ts",
              line_start: 42,
              line_end: 58,
              snippet: "export function resolve() {}",
              score: 0.95,
              // symbol_name omitted
            },
          ],
        });
        const host = createAgentHost();
        const { run } = await startRun(host);
        finishRun();
        await run;

        const sentText = mocks.sessionSend.mock.calls[0]?.[1] as string;
        expect(sentText).toContain("src/lib/workspace.ts:42-58");
        expect(sentText).not.toContain("()");

        await disposeAgentHost(host);
      });

      it("says 'Oracle found no matching files' when there are zero hits", async () => {
        mocks.oracleAsk.mockResolvedValueOnce({
          query: "nonexistent",
          results: [],
        });
        const host = createAgentHost();
        const { run } = await startRun(host);
        finishRun();
        await run;

        const sentText = mocks.sessionSend.mock.calls[0]?.[1] as string;
        expect(sentText).toContain("Oracle found no matching files.");

        await disposeAgentHost(host);
      });

      it("places output constraints before the doctrine block and the restatement after it", async () => {
        const source = builtInSkillSources()[0];
        if (source === undefined) throw new Error("No built-in doctrine source was loaded");
        const parsed = parseSkillFile(source.path, source.text);
        if (!parsed.ok) throw new Error(`Built-in doctrine did not parse: ${source.path}`);

        const host = createAgentHost();
        const { run } = await startRun(host);
        finishRun();
        await run;

        const sentText = mocks.sessionSend.mock.calls[0]?.[1] as string;
        const doctrineStart = sentText.indexOf(DESIGN_DOCTRINE_BEGIN);
        const doctrineEnd = sentText.indexOf(DESIGN_DOCTRINE_END);
        const restatementPos = sentText.indexOf(DESIGN_DOCTRINE_RESTATEMENT);

        expect(sentText).toContain(`## ${parsed.section.title}`);
        expect(doctrineStart).toBeGreaterThan(-1);
        expect(doctrineEnd).toBeGreaterThan(doctrineStart);

        // Each output constraint appears before the doctrine block.
        for (const constraint of [
          "When you produce visual output, include a self-contained HTML fragment that renders the generated design.",
          "Put it in a single fenced ```html code block. Use inline CSS for all styling.",
          "Scripts will not run, so do not rely on JavaScript — use only HTML and CSS.",
          "If you produce more than one block, only the last one is used.",
        ]) {
          expect(sentText.indexOf(constraint)).toBeGreaterThan(-1);
          expect(sentText.indexOf(constraint)).toBeLessThan(doctrineStart);
        }

        // The restatement appears after the doctrine block.
        expect(restatementPos).toBeGreaterThan(doctrineEnd);

        await disposeAgentHost(host);
      });

      it("neutralizes a doctrine delimiter before embedding the block", () => {
        const syntheticDoctrine = `## Synthetic\n\nA body containing ${DESIGN_DOCTRINE_END} cannot close the fence.`;
        const prompt = groundedPrompt("Update the design", [], syntheticDoctrine);

        expect(prompt).toContain(DESIGN_DOCTRINE_BEGIN);
        expect(prompt).toContain(DESIGN_DOCTRINE_END);
        expect(prompt).toContain("[delimiter removed]");
        expect(prompt.split(DESIGN_DOCTRINE_END)).toHaveLength(2);
      });

      it("omits the doctrine fence and restatement when the composed block is empty", () => {
        const prompt = groundedPrompt("Update the design", [], "");

        expect(prompt).not.toContain(DESIGN_DOCTRINE_BEGIN);
        expect(prompt).not.toContain(DESIGN_DOCTRINE_END);
        expect(prompt).not.toContain(DESIGN_DOCTRINE_RESTATEMENT);
      });
    });

    describe("integration: artifact in generation result", () => {
      it("carries the artifact HTML from the assistant text to the result", async () => {
        const host = createAgentHost();
        const { run } = await startRun(host);

        channelHarness.active?.({
          type: "agent_message",
          messageId: "m-1",
          text: 'Here is the generated design:\n```html\n<div class="card">Hello</div>\n```',
        });
        emitToolCall("write-1", "completed", "edit", ["src/comp.tsx"]);
        finishRun();

        const result = await run;
        expect(result.artifactHtml).toBe('<div class="card">Hello</div>');
        expect(result.sources).toEqual(["src/comp.tsx"]);

        await disposeAgentHost(host);
      });

      it("does not include an artifact when the agent errors after emitting a block", async () => {
        const host = createAgentHost();
        const { run } = await startRun(host);

        channelHarness.active?.({
          type: "agent_message",
          messageId: "m-1",
          text: "Here is the generated design:\n```html\n<div>Lost</div>\n```",
        });
        channelHarness.active?.({ type: "exit", code: 1 });

        await expect(run).rejects.toThrow("The agent stopped before finishing this turn.");
        await disposeAgentHost(host);
      });

      it("does not include an artifact when there is no fenced block in the reply", async () => {
        const host = createAgentHost();
        const { run } = await startRun(host);

        channelHarness.active?.({
          type: "agent_message",
          messageId: "m-1",
          text: "No HTML artifact here, just a description.",
        });
        finishRun();

        const result = await run;
        expect(result.artifactHtml).toBeUndefined();
        await disposeAgentHost(host);
      });

      it("returns an explicit state when the artifact exceeds the display limit", async () => {
        const host = createAgentHost();
        const { run } = await startRun(host);
        const oversizedHtml = `<div>${"x".repeat(MAX_ARTIFACT_BYTES)}</div>`;
        channelHarness.active?.({
          type: "agent_message",
          messageId: "m-oversized",
          text: `Generated:\n\`\`\`html\n${oversizedHtml}\n\`\`\``,
        });
        finishRun();

        const result = await run;
        expect(result.artifactHtml).toBeUndefined();
        expect(result.artifactError).toBe("Artifact too large to display (maximum 256 KiB).");
        await disposeAgentHost(host);
      });
    });
  });
});
