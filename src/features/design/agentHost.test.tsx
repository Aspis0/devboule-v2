// @vitest-environment happy-dom

import { StrictMode, act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  OracleIndexStatus,
  OracleSearchResponse,
  ProviderInfo,
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
  createSessionStateChannel: vi.fn(),
  sessionAttach: vi.fn(),
  sessionSend: vi.fn(),
  sessionSetModel: vi.fn(),
  sessionInterrupt: vi.fn(),
  sessionDetach: vi.fn(),
  sessionClose: vi.fn(),
  sessionPermissionRespond: vi.fn(),
  pluginsList: vi.fn(),
  providersList: vi.fn(),
  sessionsList: vi.fn(),
  sessionsUnwatch: vi.fn(),
  sessionsWatch: vi.fn(),
  surfaceSettingsGet: vi.fn(),
  surfaceSettingsSet: vi.fn(),
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
  createSessionStateChannel: mocks.createSessionStateChannel,
  sessionAttach: mocks.sessionAttach,
  sessionSend: mocks.sessionSend,
  sessionSetModel: mocks.sessionSetModel,
  sessionInterrupt: mocks.sessionInterrupt,
  sessionDetach: mocks.sessionDetach,
  sessionClose: mocks.sessionClose,
  sessionPermissionRespond: mocks.sessionPermissionRespond,
  pluginsList: mocks.pluginsList,
  providersList: mocks.providersList,
  sessionsList: mocks.sessionsList,
  sessionsUnwatch: mocks.sessionsUnwatch,
  sessionsWatch: mocks.sessionsWatch,
  // DesignSurface loads its persisted settings on mount; the mock must answer
  // in the wrapper's SurfaceSettingsRead shape. Absent keeps every assertion
  // here running against defaults, as the swallowed read did before.
  surfaceSettingsGet: mocks.surfaceSettingsGet,
  surfaceSettingsSet: mocks.surfaceSettingsSet,
}));

vi.mock("../../features/workspace/Workspace", () => ({
  Workspace: () => <div data-screen-label="Workspace">Workspace</div>,
}));

import { App } from "../../app/App";
import { useAppStore } from "../../store/appStore";
import type { AgentSessionState } from "../../lib/agentSession";
import type { DesignGenerationOptions, DesignGenerationResult } from "./designHost";
import { builtInSkillIndex, builtInSkillSources } from "./builtInSkills";
import { rankSkillsForQuery } from "./skillRanking";
import {
  buildSkillBlock,
  DOCTRINE_DESCRIPTION_CEILING_CHARS,
  parseSkillFile,
  TRUNCATION_NOTICE,
} from "./skillLoader";
import {
  automaticSkillPrompt,
  DESIGN_DOCTRINE_BEGIN,
  DESIGN_DOCTRINE_END,
  DESIGN_DOCTRINE_RESTATEMENT,
  createAgentHost,
  disposeAgentHost,
  extractArtifactHtml,
  extractFencedHtml,
  groundedPrompt,
  AUTO_SKILL_PREFLIGHT_TIMEOUT_MS,
  AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS,
  composeAutomaticSkillSlugs,
  MAX_AUTOMATIC_SKILL_SECTIONS,
  MAX_AUTOMATIC_ROUTED_SKILL_SECTIONS,
  parseAutomaticSkillReply,
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
  isolation: "local",
  path: "C:/devboule",
};
const SESSION: Session = {
  id: "session-1",
  workspaceId: WORKSPACE.id,
  kind: "acp",
  title: "Design agent",
  peerSessionId: "peer-session-1",
  createdAtMs: 1_000,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
};

function providerInfo(id: string): ProviderInfo {
  return {
    id,
    executable: id,
    acpAvailable: true,
    authentication: "unknown",
    protocol: "acp",
    origin: "user-binary",
  };
}

function sessionRecord(id: string): Session {
  return {
    ...SESSION,
    id,
    peerSessionId: `${id}-peer`,
  };
}

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

const READY_STATUS = {
  state: "ready",
  indexed_files: 1,
  model: { state: "ready" },
  reranker: null,
} as OracleIndexStatus;

const EXPECTED_PRIORITY_HEAD = ["anti-ai-slop", "typography", "color", "accessibility"] as const;
const EXPECTED_PRIORITY_HEAD_SET = new Set<string>(EXPECTED_PRIORITY_HEAD);

function expectPriorityHead(prompt: string): void {
  const index = builtInSkillIndex();
  for (const slug of EXPECTED_PRIORITY_HEAD) {
    const entry = index.find((candidate) => candidate.slug === slug);
    if (entry === undefined) throw new Error(`Expected built-in skill missing: ${slug}`);
    expect(prompt).toContain(`## ${entry.title}`);
  }
  for (const entry of index) {
    if (!EXPECTED_PRIORITY_HEAD_SET.has(entry.slug)) {
      expect(prompt).not.toContain(`## ${entry.title}`);
    }
  }
}

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
  options?: DesignGenerationOptions,
  prompt = "Update the design",
): Promise<{ run: Promise<DesignGenerationResult> }> {
  const run = host.generate?.(prompt, new AbortController().signal, options);
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
  mocks.sessionSetModel.mockReset();
  mocks.sessionInterrupt.mockReset();
  mocks.sessionDetach.mockReset();
  mocks.sessionClose.mockReset();
  mocks.sessionPermissionRespond.mockReset();
  mocks.pluginsList.mockReset();
  mocks.providersList.mockReset();
  mocks.surfaceSettingsGet.mockReset();
  mocks.surfaceSettingsSet.mockReset();

  mocks.surfaceSettingsGet.mockResolvedValue({ status: "absent" });
  mocks.surfaceSettingsSet.mockResolvedValue(undefined);

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
  mocks.sessionSetModel.mockResolvedValue(undefined);
  mocks.sessionInterrupt.mockResolvedValue(undefined);
  mocks.sessionDetach.mockResolvedValue(undefined);
  mocks.sessionClose.mockResolvedValue(undefined);
  mocks.pluginsList.mockResolvedValue({ root: "", plugins: [], problem: null });
  mocks.providersList.mockResolvedValue({ providers: [], unreadableDirs: 0 });
});

afterEach(async () => {
  const host = useAppStore.getState().designSession.host;
  if (host !== null) await disposeAgentHost(host);
  useAppStore.getState().clearDesignSession(host ?? undefined);
  document.body.replaceChildren();
});

describe("ACP design host", () => {
  it("creates the selected provider with the shared session kind mapping", async () => {
    const selectedProvider: ProviderInfo = {
      id: "grok",
      executable: "grok",
      acpAvailable: true,
      authentication: "unknown",
      protocol: "acp",
      origin: "user-binary",
    };
    const host = createAgentHost();
    host.selectProvider?.(selectedProvider);
    const { run } = await startRun(host);

    expect(mocks.sessionCreate).toHaveBeenCalledWith(null, "acp", "grok");
    channelHarness.active?.({ type: "agent_finished", stopReason: "end_turn" });
    await expect(run).resolves.toMatchObject({ title: "Agent did not report written files" });
  });

  it("opens and attaches a session immediately after provider selection", async () => {
    const host = createAgentHost();
    host.selectProvider?.(providerInfo("grok"));

    await vi.waitFor(() => expect(mocks.sessionAttach).toHaveBeenCalledTimes(1));

    expect(mocks.oracleAsk).not.toHaveBeenCalled();
    expect(mocks.sessionSend).not.toHaveBeenCalled();
    expect(host.getAgentSessionRecord?.()?.id).toBe(SESSION.id);
    expect(host.getAgentSession?.()?.getState().status).toBe("idle");

    await disposeAgentHost(host);
  });

  it("does not create a second session when the same provider is selected twice", async () => {
    const host = createAgentHost();
    const provider = providerInfo("grok");
    host.selectProvider?.(provider);
    await vi.waitFor(() => expect(mocks.sessionAttach).toHaveBeenCalledTimes(1));

    host.selectProvider?.({ ...provider });
    await Promise.resolve();

    expect(mocks.sessionCreate).toHaveBeenCalledTimes(1);
    expect(mocks.sessionAttach).toHaveBeenCalledTimes(1);
    expect(host.getAgentSessionRecord?.()?.id).toBe(SESSION.id);

    await disposeAgentHost(host);
  });

  it("closes the old provider session before the replacement becomes current", async () => {
    const host = createAgentHost();
    const first = providerInfo("provider-a");
    const second = providerInfo("provider-b");
    mocks.sessionCreate
      .mockResolvedValueOnce(sessionRecord("session-a"))
      .mockResolvedValueOnce(sessionRecord("session-b"));

    host.selectProvider?.(first);
    await vi.waitFor(() => expect(mocks.sessionAttach).toHaveBeenCalledTimes(1));
    host.selectProvider?.(second);

    await vi.waitFor(() => expect(mocks.sessionClose).toHaveBeenCalledWith("session-a"));
    await vi.waitFor(() => expect(mocks.sessionAttach).toHaveBeenCalledTimes(2));

    expect(mocks.sessionDetach).toHaveBeenCalledWith("session-a");
    expect(host.getAgentSessionRecord?.()?.id).toBe("session-b");
    expect(mocks.sessionCreate).toHaveBeenCalledTimes(2);

    await disposeAgentHost(host);
  });

  it("lets the last rapid provider selection win and closes the stale slow create", async () => {
    const firstCreate = deferred<Session>();
    const secondCreate = deferred<Session>();
    const host = createAgentHost();
    mocks.sessionCreate.mockImplementation((...args: unknown[]) => {
      if (args[2] === "provider-a") return firstCreate.promise;
      if (args[2] === "provider-b") return secondCreate.promise;
      throw new Error(`unexpected provider: ${String(args[2])}`);
    });

    host.selectProvider?.(providerInfo("provider-a"));
    await vi.waitFor(() => expect(mocks.sessionCreate).toHaveBeenCalledWith(null, "acp", "provider-a"));

    host.selectProvider?.(providerInfo("provider-b"));
    await vi.waitFor(() => expect(mocks.sessionCreate).toHaveBeenCalledWith(null, "acp", "provider-b"));

    secondCreate.resolve(sessionRecord("session-b"));
    await vi.waitFor(() => expect(mocks.sessionAttach).toHaveBeenCalledTimes(1));
    expect(host.getAgentSessionRecord?.()?.id).toBe("session-b");

    firstCreate.resolve(sessionRecord("session-a"));
    await vi.waitFor(() => expect(mocks.sessionClose).toHaveBeenCalledWith("session-a"));
    expect(mocks.sessionAttach).toHaveBeenCalledTimes(1);
    expect(host.getAgentSessionRecord?.()?.id).toBe("session-b");

    await disposeAgentHost(host);
  });

  it("leaves a failed provider start empty and allows the same selection to retry", async () => {
    const host = createAgentHost();
    const provider = providerInfo("grok");
    mocks.sessionCreate
      .mockRejectedValueOnce(new Error("provider unavailable"))
      .mockResolvedValueOnce(sessionRecord("session-retry"));

    host.selectProvider?.(provider);
    await vi.waitFor(() => expect(mocks.reasonFromCause).toHaveBeenCalledWith(expect.any(Error)));
    expect(host.getAgentSession?.()).toBeNull();
    expect(host.getAgentSessionRecord?.()).toBeNull();

    host.selectProvider?.({ ...provider });
    await vi.waitFor(() => expect(mocks.sessionAttach).toHaveBeenCalledTimes(1));

    expect(mocks.sessionCreate).toHaveBeenCalledTimes(2);
    expect(host.getAgentSessionRecord?.()?.id).toBe("session-retry");

    await disposeAgentHost(host);
  });

  it("reuses the session opened by provider selection during generation", async () => {
    const host = createAgentHost();
    host.selectProvider?.(providerInfo("grok"));
    await vi.waitFor(() => expect(mocks.sessionAttach).toHaveBeenCalledTimes(1));

    const { run } = await startRun(host);
    expect(mocks.sessionCreate).toHaveBeenCalledTimes(1);
    finishRun();
    await expect(run).resolves.toMatchObject({ sessionId: SESSION.id });

    await disposeAgentHost(host);
  });

  it("keeps the selected provider when Generate is ahead of session creation", async () => {
    const providerA: ProviderInfo = {
      id: "provider-a",
      executable: "provider-a",
      acpAvailable: true,
      authentication: "unknown",
      protocol: "acp",
      origin: "user-binary",
    };
    const providerB: ProviderInfo = {
      ...providerA,
      id: "provider-b",
      executable: "provider-b",
    };
    let releaseOracle: ((response: OracleSearchResponse) => void) | undefined;
    mocks.oracleAsk.mockImplementation(
      () =>
        new Promise((resolve) => {
          releaseOracle = resolve;
        }),
    );
    const host = createAgentHost();
    host.selectProvider?.(providerA);
    const run = host.generate?.("Update the design", new AbortController().signal);

    await vi.waitFor(() => expect(mocks.oracleAsk).toHaveBeenCalledWith("Update the design"));
    host.selectProvider?.(providerB);
    releaseOracle?.({ query: "Update the design", results: [] });

    await vi.waitFor(() => expect(mocks.sessionSend).toHaveBeenCalledTimes(1));
    expect(mocks.sessionCreate).toHaveBeenCalledWith(null, "acp", "provider-a");
    finishRun();
    await expect(run).resolves.toMatchObject({ title: "Agent did not report written files" });
  });

  it("routes model and effort changes through the existing session controller", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);
    const session = host.getAgentSession?.();
    if (session === null || session === undefined) throw new Error("agent session missing");

    await session.setModel("grok-4", "high");

    expect(mocks.sessionSetModel).toHaveBeenCalledWith("session-1", "grok-4", "high");
    channelHarness.active?.({ type: "agent_finished", stopReason: "end_turn" });
    await expect(run).resolves.toMatchObject({ title: "Agent did not report written files" });
  });

  it("preflights automatic craft selection and uses the returned sections", async () => {
    const index = builtInSkillIndex();
    const baseline = index.find((entry) =>
      AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS.includes(
        entry.slug as (typeof AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS)[number],
      ),
    );
    const selected = index.find((entry) => entry.slug !== AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS[0]);
    const omitted = index.find(
      (entry) =>
        entry.slug !== AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS[0] && entry.slug !== selected?.slug,
    );
    if (baseline === undefined || selected === undefined || omitted === undefined) {
      throw new Error("Built-in skills missing");
    }

    const host = createAgentHost();
    const { run } = await startRun(host, { skillMode: "auto" });
    const preflight = mocks.sessionSend.mock.calls[0]?.[1] as string;
    expect(preflight).toContain("Update the design");
    expect(preflight).toContain(`Already included automatically (not a choice): ${baseline.slug}`);
    expect(preflight).not.toContain(`- ${baseline.slug}: ${baseline.title}`);
    expect(preflight).toContain(selected.slug);
    expect(preflight).toContain(selected.title);
    expect(preflight).toContain(selected.description);
    expect(preflight).toContain("Do not investigate");
    expect(preflight).toContain("one line");
    expect(preflight).toContain(`at most ${MAX_AUTOMATIC_ROUTED_SKILL_SECTIONS} remaining`);
    expect(preflight).toContain("most important first");
    expect(preflight).toContain(
      `total of ${MAX_AUTOMATIC_SKILL_SECTIONS} sections. Choose fewer when fewer sections apply`,
    );
    expect(preflight).toContain("do not fill the quota just to reach the limit");

    channelHarness.active?.({
      type: "agent_message",
      messageId: "preflight-message",
      text: `I choose ${selected.slug}, unknown-future-section.`,
    });
    finishRun();
    await vi.waitFor(() => expect(mocks.sessionSend).toHaveBeenCalledTimes(2));

    const generationPrompt = mocks.sessionSend.mock.calls[1]?.[1] as string;
    expect(generationPrompt).toContain(`## ${selected.title}`);
    expect(generationPrompt).not.toContain(`## ${omitted.title}`);
    finishRun();

    const result = await run;
    expect(result.appliedSkillSlugs).toEqual([baseline.slug, selected.slug]);
    expect(result.skillSelectionFallback).toBe(false);
    await disposeAgentHost(host);
  });

  it("creates the session in the explicitly selected workspace", async () => {
    const host = createAgentHost();
    host.selectWorkspace?.(WORKSPACE);
    const { run } = await startRun(host);

    expect(mocks.sessionCreate).toHaveBeenCalledWith(WORKSPACE.id, "acp");
    finishRun();
    await run;
    await disposeAgentHost(host);
  });

  it("keeps the daemon-directory fallback when no workspace is selected", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host);

    expect(mocks.sessionCreate).toHaveBeenCalledWith(null, "acp");
    finishRun();
    await expect(run).resolves.toMatchObject({ sessionId: SESSION.id });
    await disposeAgentHost(host);
  });

  it("refuses a workspace change after a session exists", async () => {
    const host = createAgentHost();
    host.selectWorkspace?.(WORKSPACE);
    const { run } = await startRun(host);
    host.selectWorkspace?.(null);

    expect(mocks.sessionCreate).toHaveBeenCalledWith(WORKSPACE.id, "acp");
    finishRun();
    await run;
    await disposeAgentHost(host);
  });

  it("applies the baseline first without duplicating it or spending a routed slot", async () => {
    const index = builtInSkillIndex();
    const baseline = index.find((entry) => entry.slug === AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS[0]);
    const routable = index.filter((entry) => entry.slug !== baseline?.slug);
    // Name one more than the routed limit so the cap is exercised whatever the limit is.
    const named = routable.slice(0, MAX_AUTOMATIC_ROUTED_SKILL_SECTIONS + 1);
    const kept = named.slice(0, MAX_AUTOMATIC_ROUTED_SKILL_SECTIONS);
    if (baseline === undefined || named.length <= MAX_AUTOMATIC_ROUTED_SKILL_SECTIONS) {
      throw new Error("Built-in skills missing");
    }

    const host = createAgentHost();
    const { run } = await startRun(host, { skillMode: "auto" });
    channelHarness.active?.({
      type: "agent_message",
      messageId: "preflight-message",
      text: [baseline.slug, ...named.map((entry) => entry.slug)].join(", "),
    });
    finishRun();
    await vi.waitFor(() => expect(mocks.sessionSend).toHaveBeenCalledTimes(2));
    finishRun();

    const result = await run;
    expect(result.appliedSkillSlugs).toEqual([baseline.slug, ...kept.map((entry) => entry.slug)]);
    expect(result.appliedSkillSlugs?.filter((slug) => slug === baseline.slug)).toHaveLength(1);
    await disposeAgentHost(host);
  });

  it("derives the routed automatic limit from the total and baseline count", () => {
    expect(MAX_AUTOMATIC_ROUTED_SKILL_SECTIONS).toBe(
      MAX_AUTOMATIC_SKILL_SECTIONS - AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS.length,
    );
    expect(
      composeAutomaticSkillSlugs(
        [AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS[0], "typography", "color"],
        ["anti-ai-slop", "typography", "color"],
      ),
    ).toEqual(["anti-ai-slop", "typography", "color"]);
  });

  it("keeps the automatic cap reachable within the doctrine budget", () => {
    const sources = builtInSkillSources();
    const baselineSlugs = new Set<string>(AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS);
    const routedCount = MAX_AUTOMATIC_SKILL_SECTIONS - AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS.length;
    const largestNonBaseline = builtInSkillIndex()
      .filter((entry) => !baselineSlugs.has(entry.slug))
      .map((entry) => ({
        slug: entry.slug,
        totalChars: buildSkillBlock(sources, [entry.slug]).totalChars,
      }))
      .sort((left, right) => right.totalChars - left.totalChars)
      .slice(0, routedCount)
      .map((entry) => entry.slug);

    const result = buildSkillBlock(sources, [
      ...AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS,
      ...largestNonBaseline,
    ]);

    // Protect the automatic cap from becoming larger than the budget can carry.
    expect(result.dropped).toEqual([]);
    expect(result.text).not.toContain(TRUNCATION_NOTICE);
  });

  it("falls back to requesting every section when automatic selection names none", async () => {
    const index = builtInSkillIndex();
    const host = createAgentHost();
    const { run } = await startRun(host, { skillMode: "auto" });
    channelHarness.active?.({
      type: "agent_message",
      messageId: "preflight-message",
      text: "No section applies.",
    });
    finishRun();
    await vi.waitFor(() => expect(mocks.sessionSend).toHaveBeenCalledTimes(2));

    const generationPrompt = mocks.sessionSend.mock.calls[1]?.[1] as string;
    expectPriorityHead(generationPrompt);
    finishRun();

    const result = await run;
    expect(result.appliedSkillSlugs).toEqual(index.map((entry) => entry.slug));
    expect(result.skillSelectionFallback).toBe(true);
    await disposeAgentHost(host);
  });

  it("falls back to requesting every section when the automatic question errors", async () => {
    const index = builtInSkillIndex();
    mocks.sessionSend.mockRejectedValueOnce(new Error("preflight unavailable"));
    const host = createAgentHost();
    const { run } = await startRun(host, { skillMode: "auto" });

    await vi.waitFor(() => expect(mocks.sessionSend).toHaveBeenCalledTimes(2));
    const generationPrompt = mocks.sessionSend.mock.calls[1]?.[1] as string;
    expectPriorityHead(generationPrompt);
    finishRun();

    const result = await run;
    expect(result.appliedSkillSlugs).toEqual(index.map((entry) => entry.slug));
    expect(result.skillSelectionFallback).toBe(true);
    await disposeAgentHost(host);
  });

  it("preserves the agent's automatic ranking in a chatty answer", async () => {
    const index = builtInSkillIndex();
    const routed = index.filter(
      (entry) => !AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS.includes(entry.slug as "anti-ai-slop"),
    );
    const first = routed[0];
    const second = routed[1];
    if (first === undefined || second === undefined) throw new Error("Built-in skills missing");

    const host = createAgentHost();
    const { run } = await startRun(host, { skillMode: "auto" });
    channelHarness.active?.({
      type: "agent_message",
      messageId: "preflight-message",
      text: `I considered the request and recommend ${second.slug}, then ${first.slug}.`,
    });
    finishRun();
    await vi.waitFor(() => expect(mocks.sessionSend).toHaveBeenCalledTimes(2));
    finishRun();

    const result = await run;
    expect(result.appliedSkillSlugs).toEqual([
      AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS[0],
      second.slug,
      first.slug,
    ]);
    await disposeAgentHost(host);
  });

  it("caps automatic choices, deduplicates them, and ignores unknown slugs", () => {
    const index = ["one", "two", "three", "four", "five"].map((slug) => ({
      slug,
      title: slug,
      description: `${slug} description`,
    }));
    const replyOrder = [index[4]!, index[2]!, index[4]!, "unknown", index[0]!, index[1]!];
    const expected = [index[4]!, index[2]!, index[0]!, index[1]!]
      .slice(0, MAX_AUTOMATIC_ROUTED_SKILL_SECTIONS)
      .map((entry) => entry.slug);

    expect(
      parseAutomaticSkillReply(
        replyOrder.map((entry) => (typeof entry === "string" ? entry : entry.slug)).join(", "),
        index,
      ),
    ).toEqual(expected);
    expect(parseAutomaticSkillReply(`unknown, ${index[3]!.slug}`, index)).toEqual([index[3]!.slug]);
  });

  it("ignores clearly negated or discussed slug mentions", () => {
    const index = builtInSkillIndex();

    expect(parseAutomaticSkillReply("anti-ai-slop is not relevant; choose rtl", index)).toEqual([
      "rtl",
    ]);
    expect(parseAutomaticSkillReply("I considered anti-ai-slop, but choose rtl", index)).toEqual([
      "rtl",
    ]);
    expect(parseAutomaticSkillReply("Do not choose anti-ai-slop; recommend rtl", index)).toEqual([
      "rtl",
    ]);
  });

  it("falls back to requesting every section after the automatic preflight timeout", async () => {
    vi.useFakeTimers();
    try {
      const index = builtInSkillIndex();
      const host = createAgentHost();
      const { run } = await startRun(host, { skillMode: "auto" });

      await vi.advanceTimersByTimeAsync(AUTO_SKILL_PREFLIGHT_TIMEOUT_MS);
      await vi.waitFor(() => expect(mocks.sessionSend).toHaveBeenCalledTimes(2));
      expect(mocks.sessionInterrupt).toHaveBeenCalledWith("session-1");
      const generationPrompt = mocks.sessionSend.mock.calls[1]?.[1] as string;
      expectPriorityHead(generationPrompt);
      finishRun();

      const result = await run;
      expect(result.appliedSkillSlugs).toEqual(index.map((entry) => entry.slug));
      expect(result.skillSelectionFallback).toBe(true);
      await disposeAgentHost(host);
    } finally {
      vi.useRealTimers();
    }
  });

  it("aborts automatic selection without starting generation", async () => {
    const host = createAgentHost();
    const controller = new AbortController();
    const run = host.generate?.("Update the design", controller.signal, { skillMode: "auto" });
    await vi.waitFor(() => expect(mocks.sessionSend).toHaveBeenCalledTimes(1));

    controller.abort();

    await expect(run).rejects.toMatchObject({ name: "AbortError" });
    expect(mocks.sessionInterrupt).toHaveBeenCalledWith("session-1");
    expect(mocks.sessionSend).toHaveBeenCalledTimes(1);
    await disposeAgentHost(host);
  });

  it("does not attribute preflight tool events to the generation", async () => {
    const index = builtInSkillIndex();
    const selected = index[0];
    if (selected === undefined) throw new Error("Built-in skills missing");
    const host = createAgentHost();
    const { run } = await startRun(host, { skillMode: "auto" });
    emitToolCall("preflight-write", "completed", "edit", ["src/preflight.ts"]);
    channelHarness.active?.({
      type: "agent_message",
      messageId: "preflight-message",
      text: selected.slug,
    });
    finishRun();
    await vi.waitFor(() => expect(mocks.sessionSend).toHaveBeenCalledTimes(2));
    emitToolCall("generation-write", "completed", "edit", ["src/generated.tsx"]);
    finishRun();

    const result = await run;
    expect(result.sources).toEqual(["src/generated.tsx"]);
    await disposeAgentHost(host);
  });

  it("sends only the explicitly selected doctrine section", async () => {
    const index = builtInSkillIndex();
    const selected = index[0];
    const omitted = index[1];
    if (selected === undefined || omitted === undefined) throw new Error("Built-in skills missing");

    const host = createAgentHost();
    const { run } = await startRun(host, { skillMode: "manual", skills: [selected.slug] });
    finishRun();
    const result = await run;

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

    // A pin is reported like any other selection, and it never falls back.
    expect(result.appliedSkillSlugs).toEqual([selected.slug]);
    expect(result.skillSelectionFallback).toBe(false);

    await disposeAgentHost(host);
  });

  it("omits the doctrine block when an empty skill list is pinned", async () => {
    const host = createAgentHost();
    const { run } = await startRun(host, { skillMode: "manual", skills: [] });
    finishRun();
    const result = await run;

    const sentText = mocks.sessionSend.mock.calls[0]?.[1] as string;
    expect(sentText).not.toContain(DESIGN_DOCTRINE_BEGIN);
    expect(sentText).not.toContain(DESIGN_DOCTRINE_END);

    // An empty pin is a real selection of nothing, distinct from an absent report.
    expect(result.appliedSkillSlugs).toEqual([]);
    expect(result.skillSelectionFallback).toBe(false);

    await disposeAgentHost(host);
  });

  it("falls back to the priority head when matched mode finds nothing strong", async () => {
    const index = builtInSkillIndex();
    const host = createAgentHost();
    // "make it prettier" is the fallback side of the ranker's own calibration
    // test, so this pins the fallback contract to the same anchor.
    const { run } = await startRun(host, { skillMode: "all" }, "make it prettier");
    finishRun();
    await run;

    const sentText = mocks.sessionSend.mock.calls[0]?.[1] as string;
    expectPriorityHead(sentText);

    const result = await run;
    // The fallback requests every section in priority order and lets the
    // composed budget keep the head, exactly like the automatic fallback.
    // The fallback fact lives only in `skillSelectionFallback`: the list
    // length is the request size, not a signal.
    expect(result.appliedSkillSlugs).toEqual(index.map((entry) => entry.slug));
    expect(result.skillSelectionFallback).toBe(true);
    await disposeAgentHost(host);
  });

  it("ranks the corpus against the request when matched mode is declared, and prepends the baseline", async () => {
    const index = builtInSkillIndex();
    const prompt = "animate the drawer opening";
    const ranking = rankSkillsForQuery(prompt, index);
    // The strong-match side of the ranker's own calibration test: if this
    // ever falls back, the ranker or corpus drifted and this contract test
    // must be re-anchored, not silenced.
    expect(ranking.fallback).toBe(false);
    const baselineSlugs = new Set<string>(AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS);
    const routed = ranking.slugs
      .filter((slug) => !baselineSlugs.has(slug))
      .slice(0, MAX_AUTOMATIC_ROUTED_SKILL_SECTIONS);
    const expected = composeAutomaticSkillSlugs(
      routed,
      index.map((entry) => entry.slug),
    );

    const host = createAgentHost();
    const { run } = await startRun(host, { skillMode: "all" }, prompt);
    finishRun();

    const result = await run;
    const sentText = mocks.sessionSend.mock.calls[0]?.[1] as string;
    for (const slug of expected) {
      const entry = index.find((candidate) => candidate.slug === slug);
      if (entry === undefined) throw new Error(`Expected built-in skill missing: ${slug}`);
      expect(sentText).toContain(`## ${entry.title}`);
    }
    // The weakest-ranked section must not ride along on a matched request.
    const weakestSlug = ranking.slugs[ranking.slugs.length - 1];
    const weakest = index.find((candidate) => candidate.slug === weakestSlug);
    if (weakest === undefined) throw new Error("Built-in skills missing");
    expect(expected).not.toContain(weakest.slug);
    expect(sentText).not.toContain(`## ${weakest.title}`);

    // The baseline leads the matched selection, as it leads the automatic one.
    expect(expected[0]).toBe(AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS[0]);
    expect(result.appliedSkillSlugs).toEqual(expected);
    expect(result.skillSelectionFallback).toBe(false);
    await disposeAgentHost(host);
  });

  it("honors a pin with the full corpus in a different order, verbatim, without ranking it", async () => {
    // The slug set of matched mode, reordered: with a shape-derived mode this
    // list would silently rank (or silently pin while claiming a match). With
    // a declared mode it is a pin, honored verbatim: same slugs, reversed
    // order, the budget keeps the reversed head and the anti-AI-slop baseline
    // never enters by itself.
    const index = builtInSkillIndex();
    const pinned = [...index.map((entry) => entry.slug)].reverse();
    const baseline = index.find(
      (entry) => entry.slug === AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS[0],
    );
    if (baseline === undefined) throw new Error("Built-in skills missing");

    const host = createAgentHost();
    const { run } = await startRun(host, { skillMode: "manual", skills: pinned });
    finishRun();

    const result = await run;
    const sentText = mocks.sessionSend.mock.calls[0]?.[1] as string;
    // The reversed head is what fits: the first reversed sections are in, the
    // baseline — last in the reversed order — is not.
    expect(sentText).toContain(`## ${index[index.length - 1]!.title}`);
    expect(sentText).not.toContain(`## ${baseline.title}`);
    // The report states the pin verbatim and never claims a match happened.
    expect(result.appliedSkillSlugs).toEqual(pinned);
    expect(result.skillSelectionFallback).toBe(false);
    await disposeAgentHost(host);
  });

  it("defaults to matched mode when no options are sent", async () => {
    const index = builtInSkillIndex();
    const prompt = "animate the drawer opening";
    const host = createAgentHost();
    const { run } = await startRun(host, undefined, prompt);
    finishRun();

    const result = await run;
    const ranking = rankSkillsForQuery(prompt, index);
    const baselineSlugs = new Set<string>(AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS);
    const routed = ranking.slugs
      .filter((slug) => !baselineSlugs.has(slug))
      .slice(0, MAX_AUTOMATIC_ROUTED_SKILL_SECTIONS);
    expect(result.appliedSkillSlugs).toEqual(
      composeAutomaticSkillSlugs(
        routed,
        index.map((entry) => entry.slug),
      ),
    );
    expect(result.skillSelectionFallback).toBe(false);
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
    // No selection is intentional: the daemon-directory fallback remains a valid generation.
    expect(mocks.sessionCreate).toHaveBeenCalledWith(null, "acp");
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

  it("falls back to the daemon working directory when no workspace is selected", async () => {
    const host = createAgentHost();

    const { run } = await startRun(host);
    expect(mocks.oracleAsk).toHaveBeenCalledWith("Update the design");
    expect(mocks.sessionCreate).toHaveBeenCalledWith(null, "acp");
    finishRun();
    await run;

    await disposeAgentHost(host);
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

  it("loads the workspace registry, reaches ACP, and omits the debug disclosure", async () => {
    mocks.oracleStatus.mockResolvedValue(READY_STATUS);
    mocks.providersList.mockResolvedValue({
      providers: [
        {
          id: "grok",
          executable: "grok",
          acpAvailable: true,
          authentication: "unknown",
          protocol: "acp",
          origin: "user-binary",
        },
      ],
      unreadableDirs: 0,
    });
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
    expect(container.querySelector(".design-demo-disclosure")).toBeNull();
    const providerButton = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Choose provider:"]',
    );
    if (providerButton === null) throw new Error("Provider picker did not render");
    await act(async () => providerButton.click());
    const grokOption = container.querySelector<HTMLButtonElement>('[role="option"]');
    if (grokOption === null) throw new Error("Installed provider option did not render");
    await act(async () => grokOption.click());

    const draft = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Describe a design change"]',
    );
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (draft === null || send === null) throw new Error("Design composer did not render");
    const firstHost = useAppStore.getState().designSession.host;
    if (firstHost === null) throw new Error("Design host was not stored");
    const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("textarea value setter did not exist");
    setValue.call(draft, "Update the design");
    draft.dispatchEvent(new Event("input", { bubbles: true }));
    await act(async () => send.click());
    await vi.waitFor(() => expect(mocks.sessionSend).toHaveBeenCalledTimes(1));
    expect(mocks.projectsList).toHaveBeenCalled();
    expect(mocks.workspacesList).toHaveBeenCalledWith(PROJECT.id);
    expect(mocks.sessionCreate).toHaveBeenCalledWith(null, "acp", "grok");

    await act(async () => useAppStore.getState().selectSurface("workspace"));
    await settle();
    expect(mocks.sessionClose).not.toHaveBeenCalled();

    channelHarness.active?.({
      type: "agent_message",
      messageId: "artifact-message",
      text: "```html\n<main>Kept artifact</main>\n```",
    });
    finishRun();
    await vi.waitFor(() =>
      expect(useAppStore.getState().designSession.latestArtifact?.html).toBe(
        "<main>Kept artifact</main>",
      ),
    );

    const retainedHost = useAppStore.getState().designSession.host;
    expect(retainedHost).toBe(firstHost);
    await act(async () => useAppStore.getState().selectSurface("design"));
    await vi.waitFor(() =>
      expect(
        container.querySelector<HTMLTextAreaElement>(
          'textarea[aria-label="Describe a design change"]',
        ),
      ).not.toBeNull(),
    );

    expect(container.querySelector("iframe")?.getAttribute("srcdoc")).toContain(
      "<main>Kept artifact</main>",
    );
    expect(container.querySelectorAll(".design-message, .design-message-card")).toHaveLength(2);
    expect(container.textContent).toContain("Update the design");

    const secondDraft = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Describe a design change"]',
    );
    const secondSend = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (secondDraft === null || secondSend === null)
      throw new Error("Design composer did not return");
    setValue.call(secondDraft, "Make that panel green");
    secondDraft.dispatchEvent(new Event("input", { bubbles: true }));
    await act(async () => secondSend.click());
    await vi.waitFor(() => expect(mocks.sessionSend).toHaveBeenCalledTimes(2));
    channelHarness.active?.({
      type: "agent_message",
      messageId: "second-message",
      text: "The panel is green.",
    });
    finishRun();
    await settle();

    expect(mocks.sessionCreate).toHaveBeenCalledTimes(1);
    expect(mocks.sessionAttach).toHaveBeenCalledTimes(1);
    expect(mocks.oracleStatus).toHaveBeenCalledTimes(1);
    expect(container.querySelectorAll(".design-message, .design-message-card")).toHaveLength(4);

    await act(async () => root.unmount());
  });

  it("disposes the host when Design is left without any work", async () => {
    mocks.oracleStatus.mockResolvedValue(READY_STATUS);
    const { container, root } = createRootContainer();

    await act(async () => root.render(<App />));
    await vi.waitFor(() =>
      expect(
        container.querySelector<HTMLTextAreaElement>(
          'textarea[aria-label="Describe a design change"]',
        ),
      ).not.toBeNull(),
    );

    const host = useAppStore.getState().designSession.host;
    if (host === null) throw new Error("Design host was not stored");
    expect(useAppStore.getState().designSession.messages).toHaveLength(0);

    await act(async () => useAppStore.getState().selectSurface("workspace"));
    await vi.waitFor(() => expect(useAppStore.getState().designSession.host).toBeNull());

    await expect(
      host.generate?.("should be rejected", new AbortController().signal),
    ).rejects.toThrow("The design surface is no longer available.");
    await act(async () => root.unmount());
  });

  it("keeps ACP Design usable under StrictMode effect cleanup", async () => {
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

      it("bounds an oversized section description before it reaches the pre-flight", () => {
        const long = "x".repeat(DOCTRINE_DESCRIPTION_CEILING_CHARS * 4);
        const built = automaticSkillPrompt("Update the design", [
          { slug: "color", title: "Color", description: long },
        ]);

        // The strict check caps first-party descriptions; a marketplace bundle is
        // only held to it here, where its text would otherwise crowd out the user's
        // request and the instruction not to use tools.
        expect(built).not.toContain(long);
        expect(built.length).toBeLessThan(long.length);
        expect(built).toContain("[…]");
        expect(built).toContain("Update the design");
        expect(built).toContain("Do not investigate");
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
        expect(result.sessionId).toBe(SESSION.id);
        expect(result.peerSessionId).toBe(SESSION.peerSessionId);
        expect(result.createdAtMs).toBe(SESSION.createdAtMs);
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

describe("design disclosure removal", () => {
  it("does not render the old disclosure while the host is resolving", async () => {
    let resolveStatus: ((status: OracleIndexStatus) => void) | undefined;
    mocks.oracleStatus.mockReturnValue(
      new Promise((resolve) => {
        resolveStatus = resolve;
      }),
    );
    const { container, root } = createRootContainer();

    await act(async () => root.render(<App />));
    expect(container.querySelector(".design-demo-disclosure")).toBeNull();
    resolveStatus?.(READY_STATUS);
    await act(async () => root.unmount());
  });

  it("does not render the old disclosure on an ACP surface after workspace selection", async () => {
    mocks.oracleStatus.mockResolvedValue(READY_STATUS);
    const { container, root } = createRootContainer();

    await act(async () => root.render(<App />));
    await vi.waitFor(() =>
      expect(
        container.querySelector<HTMLTextAreaElement>(
          'textarea[aria-label="Describe a design change"]',
        ),
      ).not.toBeNull(),
    );
    const workspaceButton = container.querySelector<HTMLButtonElement>(
      'button[aria-label^="Choose workspace:"]',
    );
    if (workspaceButton === null) throw new Error("Workspace picker did not render");
    await act(async () => workspaceButton.click());
    const workspaceOption = Array.from(
      container.querySelectorAll<HTMLButtonElement>('[role="option"]'),
    ).find((option) => option.textContent?.includes(WORKSPACE.title));
    if (workspaceOption === undefined) throw new Error("Workspace option did not render");
    await act(async () => workspaceOption.click());

    expect(container.querySelector(".design-demo-disclosure")).toBeNull();
    await act(async () => root.unmount());
  });

  it("does not render the old disclosure on the demo fallback surface", async () => {
    mocks.oracleStatus.mockRejectedValue(new Error("Oracle daemon unavailable"));
    const { container, root } = createRootContainer();

    await act(async () => root.render(<App />));
    await act(async () => undefined);
    await act(async () => undefined);
    expect(container.querySelector(".design-demo-disclosure")).toBeNull();
    await act(async () => root.unmount());
  });
});
