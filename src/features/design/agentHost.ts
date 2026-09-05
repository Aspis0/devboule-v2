import { AgentSession, type AgentSessionState } from "../../lib/agentSession";
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
import type { Session, Workspace } from "../../types/ipc";
import type { DesignGenerationResult, DesignHost } from "./designHost";
import { createOracleHost } from "./oracleHost";

interface AgentSessionHandle {
  session: Session;
  controller: AgentSession;
  closed: boolean;
}

interface ActiveRun {
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
  let value = raw
    .trim()
    .replace(/^[([{"'`<]+/, "")
    .replace(/[\]),.;:'"`} >]+$/g, "");
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

function lastErrorText(state: AgentSessionState): string {
  for (let index = state.items.length - 1; index >= 0; index -= 1) {
    const item = state.items[index];
    if (item.role === "error") return item.text;
  }
  return "The agent session did not answer.";
}

function invokeAgentCommand<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  switch (command) {
    case "session_attach":
      return sessionAttach(
        args.id as string,
        (args.from_cursor as number | null | undefined) ?? null,
        args.ch as SessionChannel,
      ) as Promise<T>;
    case "session_send":
      return sessionSend(args.id as string, args.text as string) as Promise<T>;
    case "session_detach":
      return sessionDetach(args.id as string) as Promise<T>;
    default:
      return Promise.reject(new Error(`Unsupported agent session command: ${command}`));
  }
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
  let activeRun: ActiveRun | null = null;
  let runPending = false;
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
    if (activeRun === run) activeRun = null;
    if (outcome === "resolve") run.resolve(value as DesignGenerationResult);
    else run.reject(value);
  };

  const closeSession = async (handle: AgentSessionHandle): Promise<void> => {
    if (handle.closed) return;
    handle.closed = true;
    if (sessionHandle === handle) sessionHandle = null;
    handle.controller.dispose();
    // AgentSession.dispose() starts session_detach without awaiting it. The daemon's
    // close path (server.rs:536-541) safely accepts session_close while attached.
    try {
      await sessionClose(handle.session.id);
    } catch {
      // The surface is already gone; there is no useful UI action for cleanup failure.
    }
  };

  const openSession = async (workspace: Workspace): Promise<AgentSessionHandle> => {
    let session: Session;
    try {
      session = await sessionCreate(workspace.id, "acp");
    } catch (cause) {
      throw sessionError("Could not start the agent session", cause);
    }

    const sessionId = session.id;
    const controller = new AgentSession({
      sessionId,
      invoke: invokeAgentCommand,
      createChannel: (onEvent) =>
        createSessionChannel((event) => {
          if (event.type === "agent_tool_call" && hasFileReferenceSignal(event.title)) {
            const run = activeRun;
            if (run?.sessionId === sessionId) collectFilePaths(event.title, run.referencedFiles);
          }
          onEvent(event);
        }),
      onPermissionRequest: () => {
        const run = activeRun;
        if (run?.sessionId !== sessionId) return;
        void sessionInterrupt(sessionId).catch(() => undefined);
        settleRun(
          run,
          "reject",
          new Error(
            "The agent requested permission. Respond in the Workspace surface; this design run was stopped.",
          ),
        );
      },
    });
    const handle: AgentSessionHandle = { session, controller, closed: false };
    sessionHandle = handle;

    try {
      await controller.start();
    } catch (cause) {
      await closeSession(handle);
      if (cause instanceof DOMException && cause.name === "AbortError") throw cause;
      throw sessionError("Could not start the agent session", cause);
    }

    const state = controller.getState();
    if (state.status === "error") {
      await closeSession(handle);
      throw new Error(lastErrorText(state));
    }
    if (disposed) {
      await closeSession(handle);
      throw abortError();
    }
    return handle;
  };

  const ensureSession = async (workspace: Workspace): Promise<AgentSessionHandle> => {
    if (disposed) throw new Error("The design surface is no longer available.");
    if (sessionHandle !== null && !sessionHandle.closed) {
      if (sessionHandle.controller.getState().status !== "closed") {
        // An "error" is an agent-reported failure, not a dead session; keep it reusable.
        return sessionHandle;
      }
      await closeSession(sessionHandle);
    }
    if (sessionPromise !== null) return sessionPromise;
    const pending = openSession(workspace);
    sessionPromise = pending;
    try {
      return await pending;
    } finally {
      if (sessionPromise === pending) sessionPromise = null;
    }
  };

  const runGeneration = async (
    prompt: string,
    signal: AbortSignal,
  ): Promise<DesignGenerationResult> => {
    throwIfAborted(signal);
    const grounding = await oracleHost.generate!(prompt, signal);
    throwIfAborted(signal);
    const workspace = await resolveAgentWorkspace();
    throwIfAborted(signal);
    const handle = await ensureSession(workspace);
    throwIfAborted(signal);

    const run: ActiveRun = {
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

    // send() clears lastFinished synchronously; subscribe only after it so a prior turn cannot settle this run.
    const sendPromise = handle.controller.send(groundedPrompt(prompt, grounding.sources));
    const settleFromState = (): boolean => {
      if (activeRun !== run || run.settled) return true;
      const state = handle.controller.getState();
      if (state.lastFinished !== null) {
        settleRun(run, "resolve", resultFor(run.prompt, run.referencedFiles));
        return true;
      } else if (state.status === "error") {
        settleRun(run, "reject", new Error(lastErrorText(state)));
        return true;
      } else if (state.status === "closed") {
        settleRun(run, "reject", new Error("The agent session is closed."));
        return true;
      }
      return false;
    };
    const unsubscribe = handle.controller.subscribe(settleFromState);
    settleFromState();
    void sendPromise
      .then((sent) => {
        if (!sent && !settleFromState())
          settleRun(run, "reject", new Error("Could not send the message."));
      })
      .catch((cause: unknown) => {
        settleRun(run, "reject", sessionError("Could not send the message", cause));
      });

    try {
      return await outcomePromise;
    } finally {
      unsubscribe();
      signal.removeEventListener("abort", onAbort);
    }
  };

  // activeRun is only assigned after the Oracle, workspace and session awaits, so a
  // second call can pass a check on activeRun alone while the first is still in that
  // window. runPending is set synchronously at entry, which is what serialises runs.
  const generate = async (prompt: string, signal: AbortSignal): Promise<DesignGenerationResult> => {
    if (runPending || activeRun !== null) {
      throw new Error("A design generation is already running.");
    }
    runPending = true;
    try {
      return await runGeneration(prompt, signal);
    } finally {
      runPending = false;
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
