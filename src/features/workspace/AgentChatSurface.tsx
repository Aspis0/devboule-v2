import { memo, useEffect, useRef, useState } from "react";
import {
  createSessionChannel,
  sessionAttach,
  sessionDetach,
  sessionSend,
  type SessionChannel,
} from "../../lib/tauri";
import type { PermissionRequest, SessionManifest, SessionState } from "../../types/ipc";
import { AgentSession, type AgentChatItem, type AgentSessionState } from "../../lib/agentSession";
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
      typeof args?.from_cursor === "number" ? args.from_cursor : null,
      args?.ch as SessionChannel,
    ) as Promise<T>;
  }
  if (command === "session_send") {
    return sessionSend(id, typeof args?.text === "string" ? args.text : "") as Promise<T>;
  }
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

function manifestStrip(manifest: SessionManifest): {
  provider: string | null;
  model: string | null;
  effort: string | null;
} {
  const current = manifest.models.find((model) => model.modelId === manifest.currentModelId);
  const effort =
    current?.currentEffort && current.efforts?.some((entry) => entry.id === current.currentEffort)
      ? current.currentEffort
      : null;
  return {
    provider: manifest.providerId ?? null,
    model: current?.name ?? manifest.currentModelId ?? null,
    effort,
  };
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
  const [state, setState] = useState<AgentSessionState>({
    items: [],
    status: "initializing",
    streaming: false,
    availableCommands: [],
    lastFinished: null,
    manifest: null,
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
    const unsubscribe = session.subscribe(() => setState(session.getState()));
    void session.start();
    return () => {
      unsubscribe();
      if (sessionRef.current === session) sessionRef.current = null;
      session.dispose();
    };
  }, [onPermissionRequest, onPermissionResolved, sessionId]);

  useEffect(() => {
    const conversation = conversationRef.current;
    if (conversation === null) return;
    conversation.scrollTop = conversation.scrollHeight;
  }, [state.items, state.streaming]);

  const finishCopy = usageCopy(state);
  const strip = state.manifest === null ? null : manifestStrip(state.manifest);
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
      </div>
      {strip !== null && (strip.provider !== null || strip.model !== null) ? (
        <div className="workspace-agent-manifest" data-testid="session-manifest">
          {strip.provider !== null ? <span>{strip.provider}</span> : null}
          {strip.model !== null ? <span>{strip.model}</span> : null}
          {strip.effort !== null ? <span>{strip.effort}</span> : null}
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
