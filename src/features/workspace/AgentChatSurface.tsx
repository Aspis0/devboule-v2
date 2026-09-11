import {
  memo,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import {
  createSessionChannel,
  sessionAttach,
  sessionDetach,
  sessionInterrupt,
  sessionSend,
  sessionSetMode,
  sessionSetModel,
  type SubscriptionId,
  type SessionChannel,
} from "../../lib/tauri";
import type {
  ActiveTurnBehavior,
  PermissionRequest,
  PromptAttachment,
  SessionManifest,
  SessionModel,
  SessionState,
} from "../../types/ipc";
import { AgentSession } from "../../lib/agentSession";
import type {
  AgentChatItem,
  AgentSessionState,
  AgentSubagent,
  AgentSubagentStatusCounts,
  AgentSubagentStatus,
} from "../../lib/agentSession";
import {
  groupToolCalls,
  isToolCallGroup,
  type ToolCallGroup,
  type ToolChatItem,
} from "../../lib/toolCallGroups";
import { toolRowDisplay } from "./toolRowDisplay";
import { ToolIcon } from "./ToolIcon";
import { getPreferredEffort, setPreferredEffort } from "../../lib/modelPrefs";
import { WorkspaceComposer } from "./WorkspaceComposer";

interface AgentChatSurfaceProps {
  sessionId: string;
  title: string;
  cwd?: string;
  id?: string;
  auxiliary?: ReactNode;
  observedState?: SessionState | null;
  elapsedMs?: number | null;
  onPermissionRequest?: (
    sessionId: string,
    subscriptionId: SubscriptionId,
    request: PermissionRequest,
  ) => void;
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
    // Left off when absent, not passed as an explicit `undefined`, so a send
    // with no attachment keeps the arity every existing caller expects.
    const attachments = args?.attachments as readonly PromptAttachment[] | undefined;
    const text = typeof args?.text === "string" ? args.text : "";
    const subscriptionId = args?.subscriptionId as SubscriptionId;
    // The controller only ever names the two behaviours that differ from the
    // daemon's default; anything else on the wire is a bug and is dropped here
    // rather than typed as a behaviour the daemon does not know.
    const behavior = args?.activeTurnBehavior;
    const activeTurnBehavior: ActiveTurnBehavior | undefined =
      behavior === "queue" || behavior === "steer" ? behavior : undefined;
    if (attachments === undefined && activeTurnBehavior === undefined) {
      return sessionSend(id, subscriptionId, text) as Promise<T>;
    }
    return sessionSend(id, subscriptionId, text, attachments, activeTurnBehavior) as Promise<T>;
  }
  if (command === "session_set_model") {
    return sessionSetModel(
      id,
      typeof args?.modelId === "string" ? args.modelId : undefined,
      typeof args?.effort === "string" ? args.effort : undefined,
    ) as Promise<T>;
  }
  if (command === "session_set_mode") {
    return sessionSetMode(id, typeof args?.modeId === "string" ? args.modeId : "") as Promise<T>;
  }
  if (command === "session_interrupt")
    return sessionInterrupt(id, args?.subscriptionId as SubscriptionId) as Promise<T>;
  if (command === "session_detach")
    return sessionDetach(args?.subscriptionId as SubscriptionId) as Promise<T>;
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

const MAX_VISIBLE_SUBAGENT_DEPTH = 4;
const SUBAGENT_INDENT_PX = 16;

function hasParentToolUseId(item: AgentChatItem): boolean {
  return (
    "parentToolUseId" in item &&
    typeof item.parentToolUseId === "string" &&
    item.parentToolUseId.length > 0
  );
}

function hasMeasuredSpawnDepth(item: AgentChatItem): boolean {
  return "spawnDepth" in item && typeof item.spawnDepth === "number";
}

function subagentDepthCopy(item: AgentChatItem): string {
  return hasParentToolUseId(item) && !hasMeasuredSpawnDepth(item) ? " · depth unavailable" : "";
}

function itemLabel(item: AgentChatItem): string {
  const subagentLabel = hasParentToolUseId(item);
  const depthCopy = subagentDepthCopy(item);
  if (item.role === "user") return "You";
  if (item.role === "assistant") return subagentLabel ? `Subagent${depthCopy}` : "Agent";
  if (item.role === "thought") {
    return subagentLabel ? `Subagent thought${depthCopy}` : "Thought";
  }
  return "Error";
}

const SUBAGENT_STATUSES: AgentSubagentStatus[] = [
  "running",
  "finished",
  "failed",
  "stopped",
  "unknown",
];

function shortSubagentId(id: string): string {
  return id.length > 16 ? `${id.slice(0, 12)}…` : id;
}

function subagentTitle(title: string | null, id: string): string {
  return title?.trim() ? title : shortSubagentId(id);
}

function subagentType(type: string | null): string {
  return type?.trim() ? type : "Type unavailable";
}

function subagentDotClass(status: AgentSubagentStatus): string {
  return `workspace-subagent-status-${status}`;
}

interface SubagentMenuProps {
  subagents: AgentSubagent[];
  statusCounts: AgentSubagentStatusCounts;
}

function SubagentMenu({ subagents, statusCounts }: SubagentMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    const closeOnOutsideClick = (event: MouseEvent) => {
      const target = event.target;
      if (!(target instanceof Node) || !menuRef.current?.contains(target)) setOpen(false);
    };
    document.addEventListener("keydown", closeOnEscape);
    document.addEventListener("click", closeOnOutsideClick);
    return () => {
      document.removeEventListener("keydown", closeOnEscape);
      document.removeEventListener("click", closeOnOutsideClick);
    };
  }, [open]);

  if (subagents.length === 0) return null;

  return (
    <div ref={menuRef} className="workspace-subagent-menu">
      <button
        type="button"
        className="workspace-subagent-pill"
        data-testid="subagent-pill"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="workspace-subagent-pill-label">Subagents</span>
        {SUBAGENT_STATUSES.map((status) => {
          const count = statusCounts[status];
          if (count === 0) return null;
          return (
            <span className="workspace-subagent-pill-group" key={status}>
              <span
                className={`workspace-status-dot workspace-subagent-status-dot ${subagentDotClass(status)}`}
                aria-hidden="true"
              />
              <span>
                {count} {status}
              </span>
            </span>
          );
        })}
      </button>
      {open ? (
        <div className="workspace-subagent-list" id={listId} role="list">
          {subagents.map((subagent) => (
            <div className="workspace-subagent-row" key={subagent.id} role="listitem">
              <span
                className={`workspace-status-dot workspace-subagent-status-dot ${subagentDotClass(subagent.status)}`}
                aria-hidden="true"
              />
              <span className="workspace-subagent-row-status">{subagent.status}</span>
              <span className="workspace-subagent-type">{subagentType(subagent.subagentType)}</span>
              <span
                className="workspace-subagent-title"
                title={subagentTitle(subagent.title, subagent.id)}
              >
                {subagentTitle(subagent.title, subagent.id)}
              </span>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}

type ModeTier = "planning" | "safe" | "moderate" | "dangerous" | "neutral";

const MODE_TIERS: Record<string, ModeTier> = {
  plan: "planning",
  default: "safe",
  ask: "safe",
  acceptEdits: "moderate",
  auto: "moderate",
  "auto-edit": "moderate",
  auto_accept: "moderate",
  "auto-review": "moderate",
  bypassPermissions: "dangerous",
  bypass: "dangerous",
  yolo: "dangerous",
  "full-access": "dangerous",
};

function modeTier(modeId: string): ModeTier {
  return MODE_TIERS[modeId] ?? "neutral";
}

function modeDotClass(modeId: string): string {
  return `workspace-mode-dot workspace-mode-${modeTier(modeId)}`;
}

interface PickerOption {
  id: string;
  name: string;
  description?: string;
}

interface PickerChipProps {
  label: string;
  options: PickerOption[];
  currentId: string | null;
  onSelect: (id: string) => void;
  chipTestId: string;
  optionTestId: (id: string) => string;
  dotFor?: (id: string) => string;
}

/** One chip + listbox picker shared by the mode, model, and effort controls. */
function PickerChip({
  label,
  options,
  currentId,
  onSelect,
  chipTestId,
  optionTestId,
  dotFor,
}: PickerChipProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    const closeOnOutsideClick = (event: MouseEvent) => {
      const target = event.target;
      if (!(target instanceof Node) || !menuRef.current?.contains(target)) setOpen(false);
    };
    document.addEventListener("keydown", closeOnEscape);
    document.addEventListener("click", closeOnOutsideClick);
    return () => {
      document.removeEventListener("keydown", closeOnEscape);
      document.removeEventListener("click", closeOnOutsideClick);
    };
  }, [open]);

  if (options.length === 0) return null;

  const current = options.find((option) => option.id === currentId) ?? null;

  const handleMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const optionButtons = [
      ...(menuRef.current?.querySelectorAll<HTMLButtonElement>("[role='option']") ?? []),
    ];
    const index = optionButtons.indexOf(document.activeElement as HTMLButtonElement);
    const next =
      event.key === "ArrowDown"
        ? (optionButtons[Math.min(index + 1, optionButtons.length - 1)] ?? optionButtons[0])
        : (optionButtons[Math.max(index - 1, 0)] ?? optionButtons[0]);
    next?.focus();
  };

  return (
    <div ref={menuRef} className="workspace-mode-chip">
      <button
        type="button"
        className="workspace-mode-chip-trigger"
        data-testid={chipTestId}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        onClick={() => setOpen((value) => !value)}
      >
        {current !== null && dotFor !== undefined ? (
          <span className={dotFor(current.id)} aria-hidden="true" />
        ) : null}
        <span>{current?.name ?? currentId ?? label}</span>
        <span className="workspace-mode-caret" aria-hidden="true">
          ▾
        </span>
      </button>
      {open ? (
        <div
          className="workspace-mode-menu"
          id={listId}
          role="listbox"
          aria-label={label}
          onKeyDown={handleMenuKeyDown}
        >
          {options.map((option) => (
            <button
              type="button"
              role="option"
              className="workspace-mode-option"
              key={option.id}
              aria-selected={option.id === currentId}
              data-testid={optionTestId(option.id)}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => {
                onSelect(option.id);
                setOpen(false);
              }}
            >
              <span className="workspace-mode-name">{option.name}</span>
              {option.description ? (
                <span className="workspace-mode-description">{option.description}</span>
              ) : null}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/** Description line for a model option: catalog copy plus the context size. */
function modelOptionDescription(model: SessionModel): string | undefined {
  const parts = [
    model.description ?? null,
    model.contextTokens === undefined ? null : `${model.contextTokens.toLocaleString()} tokens`,
  ].filter((part): part is string => part !== null);
  return parts.length > 0 ? parts.join(" · ") : undefined;
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

function isToolRunningStatus(status: string): boolean {
  const normalized = status.toLowerCase();
  return normalized === "running" || normalized === "pending" || normalized === "in_progress";
}

function isToolFailedStatus(status: string): boolean {
  return status.toLowerCase() === "failed";
}

/** The frame (entry base class, subagent class, indent) the surface gives a
 * tool item. Groups compute it from their first item exactly like
 * `renderItem` does, so the wrapper aligns with its rows. */
function entryFrame(item: ToolChatItem): {
  className: string;
  style: { marginInlineStart: string } | undefined;
} {
  const isSubagent = hasParentToolUseId(item);
  const measuredDepth =
    "spawnDepth" in item && typeof item.spawnDepth === "number" ? item.spawnDepth : null;
  const visibleDepth =
    measuredDepth === null
      ? null
      : Math.min(MAX_VISIBLE_SUBAGENT_DEPTH, Math.max(0, measuredDepth));
  const className = `workspace-chat-entry workspace-chat-tool${
    isSubagent ? " workspace-chat-subagent" : ""
  }${isSubagent && measuredDepth === null ? " workspace-chat-subagent-depth-unknown" : ""}`;
  const style =
    isSubagent && visibleDepth !== null
      ? { marginInlineStart: `${visibleDepth * SUBAGENT_INDENT_PX}px` }
      : undefined;
  return { className, style };
}

function renderToolItem(
  item: ToolChatItem,
  className: string,
  style: { marginInlineStart: string } | undefined,
) {
  const model = toolRowDisplay(item);
  const running = isToolRunningStatus(item.status);
  const failed = isToolFailedStatus(item.status);
  const status = item.status.toLowerCase();
  const cancelled = status === "cancelled" || status === "canceled";
  const toolClassName = `${className}${running ? " is-running" : ""}${failed ? " is-failed" : ""}${cancelled ? " is-cancelled" : ""}`;
  return (
    <details className={toolClassName} key={item.id} style={style}>
      <summary className="workspace-chat-tool-summary">
        <ToolIcon name={model.icon} />
        <span className="workspace-chat-tool-label">{model.displayName}</span>
        {model.summary !== undefined ? (
          <span className="workspace-chat-tool-summary-text">{model.summary}</span>
        ) : null}
        {failed ? (
          <span className="workspace-chat-tool-failed" aria-hidden="true">
            ×
          </span>
        ) : null}
      </summary>
      <div className="workspace-chat-tool-body">
        {item.locations !== undefined && item.locations.length > 0 ? (
          <div className="workspace-chat-tool-locations">
            {item.locations.map((location, index) => (
              <span className="workspace-chat-tool-location" key={index}>
                {location.line !== undefined ? `${location.path}:${location.line}` : location.path}
              </span>
            ))}
          </div>
        ) : null}
        {item.output ? <div className="workspace-chat-copy">{item.output}</div> : null}
      </div>
    </details>
  );
}

/** One collapsed row for a run of consecutive tool calls (see `toolCallGroups`).
 * The wrapper carries the first item's frame, plus `is-running` when any
 * item is still running (Paseo's `isLoading`: any call running/executing)
 * and `is-failed` with the failed mark when any item failed. */
function renderGroupEntry(group: ToolCallGroup) {
  const first = group.items[0];
  if (first === undefined) return null;
  const frame = entryFrame(first);
  const running = group.items.some((item) => isToolRunningStatus(item.status));
  const failed = group.items.some((item) => isToolFailedStatus(item.status));
  const className = `${frame.className} workspace-chat-tool-group${running ? " is-running" : ""}${failed ? " is-failed" : ""}`;
  return (
    <details className={className} key={group.id} style={frame.style}>
      <summary className="workspace-chat-tool-group-summary">
        <ToolIcon name="wrench" />
        <span className="workspace-chat-tool-group-summary-text">{group.summary}</span>
        {failed ? (
          <span className="workspace-chat-tool-failed" aria-hidden="true">
            ×
          </span>
        ) : null}
      </summary>
      <div className="workspace-chat-tool-group-body">
        {group.items.map((item) => renderItem(item))}
      </div>
    </details>
  );
}

function renderEntry(entry: AgentChatItem | ToolCallGroup) {
  if (isToolCallGroup(entry)) return renderGroupEntry(entry);
  return renderItem(entry);
}

function renderItem(item: AgentChatItem) {
  const isSubagent = hasParentToolUseId(item);
  const measuredDepth =
    "spawnDepth" in item && typeof item.spawnDepth === "number" ? item.spawnDepth : null;
  const visibleDepth =
    measuredDepth === null
      ? null
      : Math.min(MAX_VISIBLE_SUBAGENT_DEPTH, Math.max(0, measuredDepth));
  const className = `workspace-chat-entry workspace-chat-${item.role}${
    isSubagent ? " workspace-chat-subagent" : ""
  }${isSubagent && measuredDepth === null ? " workspace-chat-subagent-depth-unknown" : ""}`;
  const style =
    isSubagent && visibleDepth !== null
      ? { marginInlineStart: `${visibleDepth * SUBAGENT_INDENT_PX}px` }
      : undefined;
  if (item.role === "tool") {
    const frame = entryFrame(item);
    return renderToolItem(item, frame.className, frame.style);
  }

  if (item.role === "thought") {
    return (
      <details className={className} key={item.id} open style={style}>
        <summary>{itemLabel(item)}</summary>
        <div className="workspace-chat-copy">{item.text}</div>
      </details>
    );
  }

  if (item.role === "system") {
    return (
      <div className={className} key={item.id} data-severity={item.severity} style={style}>
        <div className="workspace-chat-label">System</div>
        <div className="workspace-chat-copy">{item.text}</div>
      </div>
    );
  }

  return (
    <div
      className={className}
      key={item.id}
      role={item.role === "error" ? "alert" : undefined}
      style={style}
    >
      <div className="workspace-chat-label">{itemLabel(item)}</div>
      <div className="workspace-chat-copy">{item.text}</div>
    </div>
  );
}

export const AgentChatSurface = memo(function AgentChatSurface({
  sessionId,
  title,
  cwd,
  id,
  auxiliary,
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
    subagents: [],
    subagentStatusCounts: { running: 0, finished: 0, failed: 0, stopped: 0, unknown: 0 },
    lastFinished: null,
    manifest: null,
    pendingSwitch: null,
    pendingModeId: null,
  });
  const conversationRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const session = new AgentSession({
      sessionId,
      invoke: invokeAgentCommand,
      createChannel: createSessionChannel,
      onPermissionRequest: onPermissionRequest
        ? (request, subscriptionId) => onPermissionRequest(sessionId, subscriptionId, request)
        : undefined,
      onPermissionResolved: onPermissionResolved
        ? (toolCallId) => onPermissionResolved(sessionId, toolCallId)
        : undefined,
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
  }, [state.items, state.streaming, auxiliary]);

  const finishCopy = usageCopy(state);
  const manifest = state.manifest;
  const stripModel = manifest === null ? null : manifestModel(manifest);
  const efforts = stripModel?.efforts ?? [];
  const modes = manifest?.modes;
  const currentModeId = state.pendingModeId ?? modes?.currentModeId ?? null;
  const pendingSwitch = state.pendingSwitch !== null;
  const pendingCopy = manifest === null ? null : pendingTargetCopy(manifest, state.pendingSwitch);
  const osGone =
    observedType(observedState) === "ended" || observedType(observedState) === "recovered";
  const { copy: statusLabel, tone: statusDot } = toolbarStatus(observedState, elapsedMs, state);
  // `AgentSession` replaces the items array on every update (copy-on-write),
  // so this memo recomputes whenever the transcript changes and can never
  // go stale; it only skips work on re-renders with identical items.
  const entries = useMemo(() => groupToolCalls(state.items), [state.items]);
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
        {state.subagents.length > 0 ? (
          <SubagentMenu subagents={state.subagents} statusCounts={state.subagentStatusCounts} />
        ) : null}
        <span className="workspace-agent-status" role="status">
          {statusLabel}
        </span>
        {cwd ? <span className="workspace-session-cwd">{cwd}</span> : null}
      </div>
      {manifest !== null && (manifest.providerId !== undefined || manifest.models.length > 0) ? (
        <div
          className={`workspace-agent-manifest${pendingSwitch ? " workspace-agent-manifest-pending" : ""}`}
          data-testid="session-manifest"
          aria-busy={pendingSwitch}
        >
          {manifest.providerId !== undefined ? <span>{manifest.providerId}</span> : null}
          {pendingCopy !== null ? (
            <span data-testid="session-pending-label">{pendingCopy}</span>
          ) : null}
        </div>
      ) : null}
      <div ref={conversationRef} className="workspace-conversation workspace-scroll">
        {state.items.length === 0 && state.status === "idle" && !osGone ? (
          <div className="workspace-chat-empty">Start a conversation with the agent.</div>
        ) : null}
        {entries.map(renderEntry)}
        {state.streaming && !osGone ? (
          <div className="workspace-chat-typing" role="status">
            Agent is working
            <span className="workspace-stream-caret" aria-hidden="true" />
          </div>
        ) : null}
        {finishCopy !== null ? <div className="workspace-chat-finish">{finishCopy}</div> : null}
        {auxiliary}
      </div>
      <WorkspaceComposer
        streaming={state.streaming && !osGone}
        disabled={composerDisabled}
        disabledReason={disabledReason}
        availableCommands={state.availableCommands}
        onSend={(text) =>
          // A send while the agent is mid-turn steers that turn; an idle send
          // omits the field and the daemon keeps its interrupt-and-replace
          // default. Enter is the steering key: the send button is the Stop
          // button while the turn runs, and the textarea stays enabled.
          void sessionRef.current?.send(text, [], state.streaming ? "steer" : undefined)
        }
        onStop={() => void sessionRef.current?.interrupt()}
        controls={
          <>
            {modes !== undefined ? (
              <PickerChip
                label="Session mode"
                options={modes.availableModes.map((mode) => ({
                  id: mode.id,
                  name: mode.name,
                  description: mode.description,
                }))}
                currentId={currentModeId}
                onSelect={(modeId) => void sessionRef.current?.setMode(modeId)}
                chipTestId="mode-chip"
                optionTestId={(id) => `mode-option-${id}`}
                dotFor={modeDotClass}
              />
            ) : null}
            {manifest !== null && manifest.models.length > 1 ? (
              <PickerChip
                label="Model"
                options={manifest.models.map((model) => ({
                  id: model.modelId,
                  name: model.name,
                  description: modelOptionDescription(model),
                }))}
                currentId={manifest.currentModelId ?? null}
                onSelect={(modelId) => void sessionRef.current?.setModel(modelId)}
                chipTestId="model-chip"
                optionTestId={(id) => `model-option-${id}`}
              />
            ) : stripModel !== null ? (
              <span className="workspace-picker-static">{stripModel.name}</span>
            ) : null}
            {manifest !== null && efforts.length > 0 ? (
              <PickerChip
                label="Thinking effort"
                options={efforts.map((entry) => ({
                  id: entry.id,
                  name: entry.label,
                  description: entry.description,
                }))}
                currentId={confirmedEffort(stripModel)}
                onSelect={(effort) => {
                  if (manifest.providerId !== undefined && manifest.currentModelId !== undefined) {
                    setPreferredEffort(manifest.providerId, manifest.currentModelId, effort);
                  }
                  void sessionRef.current?.setModel(undefined, effort);
                }}
                chipTestId="effort-chip"
                optionTestId={(id) => `effort-option-${id}`}
              />
            ) : null}
          </>
        }
      />
    </div>
  );
});
