import {
  createSessionChannel,
  projectsList,
  reasonFromCause,
  sessionAttach,
  sessionClose,
  sessionCreate,
  sessionDetach,
  sessionInterrupt,
  sessionSend,
  type SessionChannel,
  workspacesList,
} from "../../lib/tauri";
import type { Session, SessionEvent, Workspace } from "../../types/ipc";
import type { DesignGenerationResult, DesignHost } from "./designHost";
import { createOracleHost } from "./oracleHost";

interface AgentSessionHandle {
  session: Session;
  channel: SessionChannel;
  attached: boolean;
  closed: boolean;
}

interface ActiveRun {
  id: number;
  sessionId: string;
  prompt: string;
  referencedFiles: Set<string>;
  settled: boolean;
  resolve: (result: DesignGenerationResult) => void;
  reject: (error: unknown) => void;
}

/**
 * SessionEvent does not expose ACP ToolCall.locations or kind; the daemon
 * flattens those fields into the tool title/text. This conservative fallback
 * scrapes likely file references from tool summaries. A tool title is stronger
 * evidence than agent_tool_update.text because it is the tool's summary rather
 * than free-form agent narration, so update text is deliberately ignored.
 * Neither signal can distinguish an affirmative write from a negation, so the
 * result says "referenced" and points to Workspace Changes for the authority
 * on the actual diff. The durable fix is to carry structured locations in the
 * daemon event.
 */
const filePathPattern =
  /(?:[A-Za-z]:[\\/])?(?:\.{0,2}[\\/])?(?:[\w@.-]+[\\/])*[\w@.-]+\.[A-Za-z0-9]+(?::\d+(?::\d+)?)?/g;
const fileBearingToolPattern =
  /\b(?:write|wrote|written|edit|edited|create|created|delete|deleted|remove|removed|move|moved|rename|renamed|patch|patched|modify|modified|replace|replaced|save|saved|apply|applied|touch|mkdir|mv|cp)\b/i;

const hostDisposers = new WeakMap<DesignHost, () => Promise<void>>();

function abortError(): DOMException {
  return new DOMException("Generation aborted", "AbortError");
}

function throwIfAborted(signal: AbortSignal): void {
  if (signal.aborted) throw abortError();
}

function normalizeFilePath(raw: string): string | null {
  let value = raw.trim().replace(/^[([{"'`<]+/, "").replace(/[\]),.;:'"`} >]+$/g, "");
  const lineSuffix = value.match(/^(.*?)(?::\d+(?::\d+)?)$/);
  if (lineSuffix?.[1]) value = lineSuffix[1];
  if (value.startsWith("file://")) value = value.slice("file://".length);
  if (!/\.[A-Za-z0-9]+$/.test(value) || /^https?:/i.test(value)) return null;
  return value.replaceAll("\\", "/");
}

function collectFilePaths(text: string | null | undefined, target: Set<string>): void {
  if (!text) return;
  for (const match of text.matchAll(filePathPattern)) {
    const path = normalizeFilePath(match[0]);
    if (path) target.add(path);
  }
}

function hasFileReferenceSignal(text: string | null | undefined): boolean {
  return text !== null && text !== undefined && fileBearingToolPattern.test(text);
}

function groundedPrompt(prompt: string, searchSources: readonly string[]): string {
  const grounding =
    searchSources.length === 0
      ? "Oracle found no matching files."
    : searchSources.map((source) => `- ${source}`).join("\n");
  return [
    "Work on the requested design change in the active Devboule workspace.",
    `User request: ${prompt}`,
    "Oracle grounding (search hits, not files changed):",
    grounding,
    "Use the grounding as context, make only the requested change, and report any file paths referenced in tool activity. The Workspace Changes panel is authoritative for the actual diff.",
  ].join("\n\n");
}

function resultFor(prompt: string, referencedFiles: Set<string>): DesignGenerationResult {
  const sources = [...referencedFiles];
  if (sources.length === 0) {
    return {
      prompt,
      title: "Agent referenced no file paths",
      desc: "The agent finished without referencing any file paths. Check Workspace Changes for the authoritative diff.",
      sources,
      nodeIds: [],
    };
  }

  const noun = sources.length === 1 ? "file" : "files";
  return {
    prompt,
    title: `Agent referenced ${sources.length} ${noun}`,
    desc: `The agent's tool activity referenced ${sources.length} ${noun}: ${sources.join(", ")}. These references do not establish what was written; check Workspace Changes for the authoritative diff.`,
    sources,
    nodeIds: [],
  };
}

function sessionError(prefix: string, cause: unknown): Error {
  return new Error(`${prefix}: ${reasonFromCause(cause)}`);
}

export async function resolveAgentWorkspace(): Promise<Workspace> {
  const projects = await projectsList();
  for (const project of projects) {
    const workspaces = await workspacesList(project.id);
    const workspace = workspaces[0];
    if (workspace) return workspace;
  }
  throw new Error("No workspace is available.");
}

export function createAgentHost(): DesignHost {
  const oracleHost = createOracleHost();
  let disposed = false;
  let nextRunId = 0;
  let activeRun: ActiveRun | null = null;
  let sessionHandle: AgentSessionHandle | null = null;
  let sessionPromise: Promise<AgentSessionHandle> | null = null;
  let disposalPromise: Promise<void> | null = null;

  const settleRun = (
    run: ActiveRun,
    outcome: "resolve" | "reject",
    value: DesignGenerationResult | unknown,
  ): void => {
    if (run.settled) return;
    run.settled = true;
    if (activeRun?.id === run.id) activeRun = null;
    if (outcome === "resolve") run.resolve(value as DesignGenerationResult);
    else run.reject(value);
  };

  const handleEvent = (event: SessionEvent): void => {
    if (disposed) return;
    const run = activeRun;
    if (run === null) return;

    switch (event.type) {
      case "agent_tool_call": {
        if (hasFileReferenceSignal(event.title)) collectFilePaths(event.title, run.referencedFiles);
        return;
      }
      case "agent_tool_update": {
        // Updates are free-form agent narration and cannot prove a file operation.
        return;
      }
      case "permission_request": {
        const message = new Error(
          "The agent requested permission. Respond in the Workspace surface; this design run was stopped.",
        );
        const handle = sessionHandle;
        if (handle) void sessionInterrupt(handle.session.id).catch(() => undefined);
        settleRun(run, "reject", message);
        return;
      }
      case "agent_finished":
        settleRun(run, "resolve", resultFor(run.prompt, run.referencedFiles));
        return;
      case "agent_error":
        settleRun(run, "reject", new Error(event.message || "The agent reported an unknown error."));
        return;
      case "exit":
        settleRun(run, "reject", new Error("The agent stopped before finishing this design run."));
        return;
      case "recovered":
        settleRun(run, "reject", new Error("The agent session is no longer available."));
        return;
      case "output":
      case "agent_message":
      case "agent_user_message":
      case "agent_thought":
      case "agent_stderr":
      case "silent":
      case "journal_degraded":
      case "sessions_snapshot":
      case "snapshot":
      case "agent_reported":
      case "available_commands":
      case "permission_resolved":
      case "session_manifest":
        return;
    }
  };

  const closeSession = async (handle: AgentSessionHandle): Promise<void> => {
    if (handle.closed) return;
    handle.closed = true;
    if (sessionHandle === handle) sessionHandle = null;
    if (handle.attached) {
      handle.attached = false;
      try {
        await sessionDetach(handle.session.id);
      } catch {
        // Closing the runtime session is still required if detaching fails.
      }
    }
    try {
      await sessionClose(handle.session.id);
    } catch {
      // The surface is already gone; there is no useful UI action for cleanup failure.
    }
  };

  const openSession = async (workspace: Workspace): Promise<AgentSessionHandle> => {
    let session: Session | null = null;
    try {
      session = await sessionCreate(workspace.id, "acp");
      const handle: AgentSessionHandle = {
        session,
        channel: createSessionChannel(handleEvent),
        attached: false,
        closed: false,
      };
      sessionHandle = handle;
      if (disposed) throw abortError();
      await sessionAttach(session.id, null, handle.channel);
      handle.attached = true;
      return handle;
    } catch (cause) {
      const failedHandle = sessionHandle;
      if (failedHandle !== null && failedHandle.session.id === session?.id) {
        await closeSession(failedHandle);
      }
      if (cause instanceof DOMException && cause.name === "AbortError") throw cause;
      throw sessionError("Could not start the agent session", cause);
    }
  };

  const ensureSession = (workspace: Workspace): Promise<AgentSessionHandle> => {
    if (disposed) return Promise.reject(new Error("The design surface is no longer available."));
    if (sessionHandle !== null && !sessionHandle.closed) return Promise.resolve(sessionHandle);
    if (sessionPromise !== null) return sessionPromise;
    sessionPromise = openSession(workspace).catch((cause: unknown) => {
      sessionPromise = null;
      throw cause;
    });
    return sessionPromise;
  };

  const generate = async (prompt: string, signal: AbortSignal): Promise<DesignGenerationResult> => {
    throwIfAborted(signal);
    const grounding = await oracleHost.generate!(prompt, signal);
    throwIfAborted(signal);
    const workspace = await resolveAgentWorkspace();
    throwIfAborted(signal);
    const handle = await ensureSession(workspace);
    throwIfAborted(signal);

    const run: ActiveRun = {
      id: ++nextRunId,
      sessionId: handle.session.id,
      prompt,
      referencedFiles: new Set<string>(),
      settled: false,
      resolve: () => undefined,
      reject: () => undefined,
    };
    activeRun = run;
    const resultPromise = new Promise<DesignGenerationResult>((resolve, reject) => {
      run.resolve = resolve;
      run.reject = reject;
    });
    let rejectAbort: (error: DOMException) => void = () => undefined;
    let interruptRequested = false;
    const abortPromise = new Promise<never>((_resolve, reject) => {
      rejectAbort = reject;
    });
    const outcomePromise = Promise.race([resultPromise, abortPromise]);
    const onAbort = (): void => {
      if (interruptRequested || run.settled) return;
      interruptRequested = true;
      void sessionInterrupt(run.sessionId).catch(() => undefined);
      const error = abortError();
      settleRun(run, "reject", error);
      rejectAbort(error);
    };
    signal.addEventListener("abort", onAbort, { once: true });

    try {
      if (signal.aborted) onAbort();
      else {
        try {
          await sessionSend(handle.session.id, groundedPrompt(prompt, grounding.sources));
        } catch (cause) {
          if (activeRun?.id === run.id) {
            settleRun(run, "reject", sessionError("Could not send the design request", cause));
          }
        }
      }
      return await outcomePromise;
    } finally {
      signal.removeEventListener("abort", onAbort);
    }
  };

  const dispose = async (): Promise<void> => {
    if (disposalPromise !== null) return disposalPromise;
    disposalPromise = (async () => {
      disposed = true;
      const run = activeRun;
      if (run !== null) {
        const handle = sessionHandle;
        if (handle) void sessionInterrupt(handle.session.id).catch(() => undefined);
        settleRun(run, "reject", abortError());
      }
      if (sessionPromise !== null) await sessionPromise.catch(() => undefined);
      if (sessionHandle !== null) await closeSession(sessionHandle);
    })();
    return disposalPromise;
  };

  const host: DesignHost = {
    loadDocument: oracleHost.loadDocument,
    generate,
  };
  hostDisposers.set(host, dispose);
  return host;
}

export function disposeAgentHost(host: DesignHost): Promise<void> {
  return hostDisposers.get(host)?.() ?? Promise.resolve();
}
