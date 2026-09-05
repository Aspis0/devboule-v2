// @vitest-environment happy-dom

import { StrictMode, act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { OracleIndexStatus, Session, SessionEvent, Workspace } from "../../types/ipc";

const channelHarness = vi.hoisted(() => ({
  emit: null as ((event: SessionEvent) => void) | null,
  active: null as ((event: SessionEvent) => void) | null,
  handlers: new WeakMap<object, (event: SessionEvent) => void>(),
}));

const mocks = vi.hoisted(() => ({
  oracleAsk: vi.fn(),
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
import type { DesignGenerationResult } from "./designHost";
import { createAgentHost, disposeAgentHost } from "./agentHost";

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

async function startRun(
  host: ReturnType<typeof createAgentHost>,
): Promise<{ run: Promise<DesignGenerationResult> }> {
  const run = host.generate?.("Update the design", new AbortController().signal);
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
  it("reports file paths referenced by agent tool summaries as sources", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);

    channelHarness.active?.({
      type: "agent_tool_call",
      toolCallId: "write-1",
      title: "Write src/features/design/DesignSurface.tsx",
      status: "running",
    });
    channelHarness.active?.({
      type: "agent_tool_update",
      toolCallId: "write-1",
      status: "completed",
      text: "Wrote src/features/design/DesignSurface.tsx",
    });
    channelHarness.active?.({
      type: "agent_tool_call",
      toolCallId: "edit-1",
      title: "Edit src/features/design/agentHost.ts",
      status: "completed",
    });
    finishRun();

    const result = await run;
    expect(result.sources).toEqual([
      "src/features/design/DesignSurface.tsx",
      "src/features/design/agentHost.ts",
    ]);
    expect(result.desc).toContain("2 files");
    expect(result.desc).toContain("DesignSurface.tsx");
    expect(result.title).toContain("referenced");
    expect(result.desc).toContain("Workspace Changes");
    expect(mocks.sessionCreate).toHaveBeenCalledWith(WORKSPACE.id, "acp");
    expect(mocks.sessionAttach).toHaveBeenCalledWith("session-1", null, expect.anything());
    expect(mocks.sessionSend).toHaveBeenCalledWith(
      "session-1",
      expect.stringContaining("src/app/Shell.tsx"),
    );

    await disposeAgentHost(host);
  });

  it("says when the agent finishes without referencing file paths", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);
    finishRun();

    const result = await run;
    expect(result.sources).toEqual([]);
    expect(result.desc).toContain("finished without referencing any file paths");

    await disposeAgentHost(host);
  });

  it("does not present a negated prose mention as a written source", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);

    channelHarness.active?.({
      type: "agent_tool_call",
      toolCallId: "edit-1",
      title: "Edit file",
      status: "running",
    });
    channelHarness.active?.({
      type: "agent_tool_update",
      toolCallId: "edit-1",
      status: "completed",
      text: "I did not modify src/features/design/agentHost.ts",
    });
    finishRun();

    const result = await run;
    expect(result.sources).not.toContain("src/features/design/agentHost.ts");
    expect(result.title).toContain("referenced");
    expect(result.desc).toContain("Workspace Changes");

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
      "Repository agent — ACP writes in the active worktree.",
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
});
