import { memo, useEffect, useRef, useState } from "react";
import {
  createSessionChannel,
  sessionAttach,
  sessionDetach,
  sessionInterrupt,
  sessionSend,
  sessionSetModel,
  type SessionChannel,
} from "../../lib/tauri";
import type {
  PermissionRequest,
  SessionManifest,
  SessionModel,
  SessionState,
} from "../../types/ipc";
import { AgentSession, type AgentChatItem, type AgentSessionState } from "../../lib/agentSession";
import { getPreferredEffort, setPreferredEffort } from "../../lib/modelPrefs";
import { WorkspaceComposer } from "./WorkspaceComposer";

interface AgentChatSurfaceProps {
  sessionId: string;
  title: string;
  id?: string;
  observedState?: SessionState | null;
  elapsedMs?: number | null;
  onPermissionRequest?: (sessionId: string, request: PermissionRequest) => void;
  onPermissionResolved?: (sessionId: string, toolCallId: string) => void;
}

function commandId(args: Record<string, unknown> | undefined): string {
  const id = args?.id;
  return typeof id === "string" ? id : "";
}

function invokeAgentCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const id = commandId(args);
  if (command === "session_attach") {
    return sessionAttach(
      id,
      typeof args?.fromCursor === "number" ? args.fromCursor : null,
      args?.ch as SessionChannel,
    ) as Promise<T>;
  }
  if (command === "session_send") {
    return sessionSend(id, typeof args?.text === "string" ? args.text : "") as Promise<T>;
  }
  if (command === "session_set_model") {
    return sessionSetModel(
      id,
      typeof args?.modelId === "string" ? args.modelId : undefined,
      typeof args?.effort === "string" ? args.effort : undefined,
    ) as Promise<T>;
  }
  if (command === "session_interrupt") return sessionInterrupt(id) as Promise<T>;
  if (command === "session_detach") return sessionDetach(id) as Promise<T>;
  return Promise.reject(new Error(`Unsupported agent command: ${command}`));
}

function formatElapsed(elapsedMs: number): string {
  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes > 0) return `${minutes} minute${minutes === 1 ? "" : "s"}`;
  const seconds = Math.floor(elapsedMs / 1_000);
  return `${seconds} second${seconds === 1 ? "" : "s"}`;
}

function observedType(state: SessionState | null | undefined): SessionState["type"] | null {
  return state?.type ?? null;
}

function toolbarStatus(
  observed: SessionState | null | undefined,
  elapsedMs: number | null | undefined,
  agent: AgentSessionState,
): { copy: string; tone: "green" | "terracotta" | "border" } {
  const type = observedType(observed);
  if (type === "ended" || type === "recovered") {
    return { copy: "Finished", tone: "terracotta" };
  }
  if (type === "silent") {
    return {
      copy: typeof elapsedMs === "number" ? `Silent for ${formatElapsed(elapsedMs)}` : "Silent",
      tone: "border",
    };
  }
  if (agent.status === "error") return { copy: "Needs attention", tone: "terracotta" };
  if (agent.status === "closed") return { copy: "Finished", tone: "terracotta" };
  if (agent.status === "running") return { copy: "Working…", tone: "green" };
  if (type === "live") return { copy: "Live", tone: "green" };
  return { copy: "Connecting…", tone: "border" };
}

function itemLabel(item: AgentChatItem): string {
  if (item.role === "user") return "You";
  if (item.role === "assistant") return "Agent";
  if (item.role === "thought") return "Thought";
  if (item.role === "tool") return `Tool · ${item.status}`;
  return "Error";
}

function usageCopy(state: AgentSessionState): string | null {
  const finished = state.lastFinished;
  if (finished === null) return null;
  const details = [
    finished.modelId ? `model ${finished.modelId}` : null,
    finished.stopReason ? `stopped: ${finished.stopReason}` : null,
    finished.usage?.inputTokens === undefined
      ? null
      : `in ${finished.usage.inputTokens.toLocaleString()}`,
    finished.usage?.outputTokens === undefined
      ? null
      : `out ${finished.usage.outputTokens.toLocaleString()}`,
    finished.usage?.thoughtTokens === undefined
      ? null
      : `thought ${finished.usage.thoughtTokens.toLocaleString()}`,
    finished.usage?.totalTokens === undefined
      ? null
      : `total ${finished.usage.totalTokens.toLocaleString()} tokens`,
  ].filter((part): part is string => part !== null);
  return details.length > 0 ? details.join(" · ") : null;
}

function manifestModel(manifest: SessionManifest): SessionModel | null {
  return manifest.models.find((model) => model.modelId === manifest.currentModelId) ?? null;
}

/** The effort the runtime confirmed, only when the model actually declares it. */
function confirmedEffort(model: SessionModel | null): string | null {
  if (
    model?.currentEffort !== undefined &&
    model.efforts?.some((entry) => entry.id === model.currentEffort)
  ) {
    return model.currentEffort;
  }
  return null;
}

/** What the strip says a pending switch is heading toward, or null. */
function pendingTargetCopy(
  manifest: SessionManifest,
  pending: { modelId?: string; effort?: string; at: number } | null,
): string | null {
  if (pending === null) return null;
  if (pending.modelId !== undefined) {
    const model = manifest.models.find((entry) => entry.modelId === pending.modelId);
    return `switching to ${model?.name ?? pending.modelId}…`;
  }
  const model = manifestModel(manifest);
  const effort = model?.efforts?.find((entry) => entry.id === pending.effort);
  return effort === undefined ? null : `switching to ${effort.label}…`;
}

function renderItem(item: AgentChatItem) {
  const className = `workspace-chat-entry workspace-chat-${item.role}`;
  if (item.role === "thought") {
    return (
      <details className={className} key={item.id} open>
        <summary>{itemLabel(item)}</summary>
        <div className="workspace-chat-copy">{item.text}</div>
      </details>
    );
  }

  return (
    <div className={className} key={item.id} role={item.role === "error" ? "alert" : undefined}>
      <div className="workspace-chat-label">{itemLabel(item)}</div>
      <div className="workspace-chat-copy">{item.text}</div>
    </div>
  );
}

export const AgentChatSurface = memo(function AgentChatSurface({
  sessionId,
  title,
  id,
  observedState = null,
  elapsedMs = null,
  onPermissionRequest,
  onPermissionResolved,
}: AgentChatSurfaceProps) {
  const sessionRef = useRef<AgentSession | null>(null);
  const appliedEffortPrefRef = useRef(false);
  const [state, setState] = useState<AgentSessionState>({
    items: [],
    status: "initializing",
    streaming: false,
    availableCommands: [],
    lastFinished: null,
    manifest: null,
    pendingSwitch: null,
  });
  const conversationRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const session = new AgentSession({
      sessionId,
      invoke: invokeAgentCommand,
      createChannel: createSessionChannel,
      onPermissionRequest: onPermissionRequest
        ? (request) => onPermissionRequest(sessionId, request)
        : undefined,
      onPermissionResolved: onPermissionResolved
        ? (toolCallId) => onPermissionResolved(sessionId, toolCallId)
        : undefined,
      isSuperseded: () => {
        const activeSession = sessionRef.current;
        return activeSession !== null && activeSession !== session;
      },
    });
    sessionRef.current = session;
    appliedEffortPrefRef.current = false;
    const unsubscribe = session.subscribe(() => setState(session.getState()));
    void session.start();
    return () => {
      unsubscribe();
      if (sessionRef.current === session) sessionRef.current = null;
      session.dispose();
    };
  }, [onPermissionRequest, onPermissionResolved, sessionId]);

  // Zed's pattern: re-apply the remembered effort once, on the first manifest
  // of the session. The confirmation manifest is just another manifest here —
  // the ref guard keeps the auto-switch from re-triggering. The preference is
  // a localStorage/product concern, so it lives on the surface next to the
  // manual onChange handler, not inside the headless session controller.
  useEffect(() => {
    const manifest = state.manifest;
    if (manifest === null || appliedEffortPrefRef.current) return;
    appliedEffortPrefRef.current = true;
    const { providerId, currentModelId } = manifest;
    if (providerId === undefined || currentModelId === undefined) return;
    const model = manifestModel(manifest);
    if (model === null || !model.efforts || model.efforts.length === 0) return;
    const stored = getPreferredEffort(providerId, currentModelId);
    if (stored === null || stored === model.currentEffort) return;
    // A stale preference (a model that no longer offers that effort) must not
    // produce a doomed switch; skip it without surfacing an error.
    if (!model.efforts.some((entry) => entry.id === stored)) return;
    void sessionRef.current?.setModel(currentModelId, stored);
  }, [state.manifest]);

  useEffect(() => {
    const conversation = conversationRef.current;
    if (conversation === null) return;
    conversation.scrollTop = conversation.scrollHeight;
  }, [state.items, state.streaming]);

  const finishCopy = usageCopy(state);
  const manifest = state.manifest;
  const stripModel = manifest === null ? null : manifestModel(manifest);
  const efforts = stripModel?.efforts ?? [];
  const pendingSwitch = state.pendingSwitch !== null;
  const pendingCopy = manifest === null ? null : pendingTargetCopy(manifest, state.pendingSwitch);
  const osGone =
    observedType(observedState) === "ended" || observedType(observedState) === "recovered";
  const { copy: statusLabel, tone: statusDot } = toolbarStatus(observedState, elapsedMs, state);
  const composerDisabled = osGone || (state.status !== "idle" && state.status !== "running");
  const disabledReason =
    state.status === "initializing" && !osGone
      ? "Connecting to the agent…"
      : "This session is no longer available.";

  return (
    <div id={id} className="workspace-agent-shell" role="tabpanel" aria-label="Agent chat">
      <div className="workspace-agent-toolbar">
        <span className={`workspace-status-dot workspace-dot-${statusDot}`} />
        <span className="workspace-agent-title">{title || "Agent"}</span>
        <span className="workspace-agent-status" role="status">
          {statusLabel}
        </span>
        {state.status === "running" ? (
          <button
            type="button"
            className="workspace-agent-stop"
            aria-label="Stop the current turn"
            onClick={() => void sessionRef.current?.interrupt()}
          >
            Stop
          </button>
        ) : null}
      </div>
      {manifest !== null && (manifest.providerId !== undefined || manifest.models.length > 0) ? (
        <div
          className={`workspace-agent-manifest${pendingSwitch ? " workspace-agent-manifest-pending" : ""}`}
          data-testid="session-manifest"
          aria-busy={pendingSwitch}
        >
          {manifest.providerId !== undefined ? <span>{manifest.providerId}</span> : null}
          {manifest.models.length > 1 ? (
            <select
              data-testid="session-model-select"
              aria-label="Model"
              value={manifest.currentModelId ?? ""}
              onChange={(event) => void sessionRef.current?.setModel(event.target.value)}
            >
              {manifest.models.map((model) => (
                <option key={model.modelId} value={model.modelId}>
                  {model.name}
                </option>
              ))}
            </select>
          ) : stripModel !== null ? (
            <span>{stripModel.name}</span>
          ) : null}
          {efforts.length > 0 ? (
            <select
              data-testid="session-effort-select"
              aria-label="Thinking effort"
              value={confirmedEffort(stripModel) ?? ""}
              onChange={(event) => {
                const effort = event.target.value;
                if (manifest.providerId !== undefined && manifest.currentModelId !== undefined) {
                  setPreferredEffort(manifest.providerId, manifest.currentModelId, effort);
                }
                void sessionRef.current?.setModel(undefined, effort);
              }}
            >
              {efforts.map((entry) => (
                <option key={entry.id} value={entry.id}>
                  {entry.label}
                </option>
              ))}
            </select>
          ) : null}
          {pendingCopy !== null ? (
            <span data-testid="session-pending-label">{pendingCopy}</span>
          ) : null}
        </div>
      ) : null}
      {state.manifest?.modes && state.manifest.modes.availableModes.length > 0 ? (
        <div
          className="workspace-agent-modes"
          role="radiogroup"
          aria-label="Session mode"
          data-testid="session-modes"
        >
          {state.manifest.modes.availableModes.map((mode) => (
            <span
              key={mode.id}
              role="radio"
              aria-checked={mode.id === state.manifest?.modes?.currentModeId}
              title={mode.description}
            >
              {mode.name}
            </span>
          ))}
        </div>
      ) : null}
      <div ref={conversationRef} className="workspace-conversation workspace-scroll">
        {state.items.length === 0 && state.status === "idle" && !osGone ? (
          <div className="workspace-chat-empty">Start a conversation with the agent.</div>
        ) : null}
        {state.items.map(renderItem)}
        {state.streaming && !osGone ? (
          <div className="workspace-chat-typing" role="status">
            Agent is working
            <span className="workspace-stream-caret" aria-hidden="true" />
          </div>
        ) : null}
        {finishCopy !== null ? <div className="workspace-chat-finish">{finishCopy}</div> : null}
      </div>
      <WorkspaceComposer
        streaming={state.streaming && !osGone}
        disabled={composerDisabled}
        disabledReason={disabledReason}
        availableCommands={state.availableCommands}
        onSend={(text) => void sessionRef.current?.send(text)}
      />
    </div>
  );
});
