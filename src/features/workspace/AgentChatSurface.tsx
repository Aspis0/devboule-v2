import { memo, useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
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
  DaemonConnectionState,
  PermissionRequest,
  PermissionResolved,
  PromptAttachment,
  Session,
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
import { boundByGraphemes } from "../../lib/graphemeBound";
import { WorkspaceComposer } from "./WorkspaceComposer";
import { journalLossCopy } from "./journalLoss";
import { PickerChip, modeDotClass } from "../../components/PickerChip";
import { DaemonNoticeCard } from "./DaemonNoticeCard";
import { A2aMessageCard, type A2aNameSource } from "./A2aMessageCard";

interface AgentChatSurfaceProps {
  sessionId: string;
  title: string;
  cwd?: string;
  id?: string;
  auxiliary?: ReactNode;
  observedState?: SessionState | null;
  elapsedMs?: number | null;
  /** The daemon connection's state; input is disabled while it cannot carry sends. Required so an omission is compile-visible. */
  daemonState: DaemonConnectionState;
  /**
   * The roster rows the agent-to-agent card resolves a relay's sender
   * against. Handed none, the card can only show the session id the frame
   * named — the truth it has, minus the name.
   */
  sessionRoster?: ReadonlyArray<Pick<Session, "displayName" | "id" | "kind" | "title">>;
  /**
   * Device id to display name, the same `DevicesList` map the permission
   * card's origin line resolves against; the a2a card resolves a relay's
   * paired device with it.
   */
  deviceNames?: ReadonlyMap<string, string>;
  onPermissionRequest?: (
    sessionId: string,
    subscriptionId: SubscriptionId,
    request: PermissionRequest,
  ) => void;
  onPermissionResolved?: (sessionId: string, resolution: PermissionResolved) => void;
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
    // The controller only ever names the one behaviour that differs from the
    // daemon's default. Anything else is a bug on this side of the wire and is
    // refused loudly: silently dropping it would turn a misspelling into an
    // interrupt-and-replace the caller never asked for.
    const behavior = args?.activeTurnBehavior;
    if (behavior !== undefined && behavior !== "steer") {
      return Promise.reject(new Error(`Unsupported active turn behavior: ${String(behavior)}`));
    }
    const activeTurnBehavior: ActiveTurnBehavior | undefined = behavior;
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

/**
 * The bound on a daemon-supplied field inside the permission-request header
 * sentence. It exists for layout only: an unbroken multi-kilobyte
 * `displayName` would push the chat pane sideways, since the sentence —
 * unlike the quoted excerpt — has no wrapping contract of its own. The FULL
 * values stay on the sentence element's `title`; the excerpt below is never
 * bounded, because never-re-truncate is the excerpt's rule and no one else's.
 */
const PERMISSION_HEADER_FIELD_LIMIT = 200;

// Bounded by grapheme clusters, not code units: a unit-based pre-check
// appends an ellipsis to a 200-unit astral name whose 100 scalars were
// already inside the bound — a truncation that did not happen — and a
// unit-based cut splits a scalar in half (re-audit F12).
function boundPermissionHeaderField(value: string): string {
  return boundByGraphemes(value, PERMISSION_HEADER_FIELD_LIMIT);
}

/**
 * The excerpt's rendering decision, one row per value of the parser's closed
 * `excerptState` vocabulary — the walked-table rule this slice applied to
 * `delegation.state` and the setting's `source`, applied to the vocabulary
 * this slice itself introduced (re-audit F7: two `===` tests made the
 * fallthrough the benign `"closed"` render, and a new member without a
 * rendering decision was neither a compile error nor a failing test). A value
 * outside the vocabulary at runtime — a mixed bundle, a refactor that missed a
 * row — takes the visible unknown-state arm: the words that did arrive still
 * show, with a note that their state could not be established, never the
 * silent render that claims the fence closed.
 */
type KnownExcerptState = Extract<AgentChatItem, { role: "permission_request" }>["excerptState"];
const EXCERPT_STATE_RENDER: Record<KnownExcerptState, { block: boolean; note: string | null }> = {
  closed: { block: true, note: null },
  unterminated: {
    block: true,
    note: "the closing fence never arrived — this block runs to the end of the frame",
  },
  // Re-audit F3: `"absent"` rendered `null` — no block, no note, no sentence
  // — while the frame's header fields still parsed, so a frame whose opener
  // was not byte-exact lost the child's words with no marker at all. The
  // absence of quoted words is its own visible fact: the frame arrived, the
  // words did not.
  absent: {
    block: false,
    note: "this frame carried no quoted block — no `child-said:` opener arrived, so none of the child's words are shown",
  },
};
const UNKNOWN_EXCERPT_STATE_RENDER: { block: boolean; note: string | null } = {
  block: true,
  note: "the quoted block's state was not recognised — these are the words the frame carried, unbounded",
};

/** Exported for the out-of-union test: the walk is the render decision, and
 * the test casts a value TypeScript cannot predict through it. */
export function excerptRenderFor(state: KnownExcerptState): {
  block: boolean;
  note: string | null;
} {
  return Object.hasOwn(EXCERPT_STATE_RENDER, state)
    ? EXCERPT_STATE_RENDER[state]
    : UNKNOWN_EXCERPT_STATE_RENDER;
}

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
function renderGroupEntry(group: ToolCallGroup, a2aNames: A2aNameSource) {
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
        {group.items.map((item) => renderItem(item, a2aNames))}
      </div>
    </details>
  );
}

function renderEntry(entry: AgentChatItem | ToolCallGroup, a2aNames: A2aNameSource) {
  if (isToolCallGroup(entry)) return renderGroupEntry(entry, a2aNames);
  return renderItem(entry, a2aNames);
}

function renderItem(item: AgentChatItem, a2aNames: A2aNameSource) {
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

  if (item.role === "permission_request") {
    const childName = boundPermissionHeaderField(item.childName);
    const toolTitle = boundPermissionHeaderField(item.toolTitle);
    const cardId = boundPermissionHeaderField(item.cardId);
    return (
      <div
        className="workspace-chat-entry workspace-chat-permission-request"
        key={item.id}
        data-testid="agent-permission-request"
      >
        {/* NOT labelled "System": this item is parsed out of session text —
            the daemon's send path in the honest case, but a pasted block is
            byte-identical to this app, and the app cannot verify who authored
            it. A chip claiming the daemon spoke would be styling making a
            verification the code never did. */}
        <div
          className="workspace-chat-label workspace-chat-label-unverified"
          title="This arrived as session text. The app cannot verify the daemon sent it."
        >
          Relayed · unverified
        </div>
        <div
          className="workspace-chat-copy"
          title={`${item.childName} · ${item.toolTitle} · ${item.cardId}`}
        >
          Its child {childName} asks to run {toolTitle} and is waiting on a permission card. Card{" "}
          {cardId} — it answers through its own tool; the card itself is on the child&apos;s
          session.
        </div>
        {(() => {
          const excerptRender = excerptRenderFor(item.excerptState);
          if (!excerptRender.block) {
            // No quoted block: the note stands alone — a blockquote here
            // would style absence as if words were quoted inside it.
            return excerptRender.note === null ? null : (
              <p className="workspace-chat-child-said-note" role="note">
                {excerptRender.note}
              </p>
            );
          }
          // The child's own words — a quoted block with its own styling and
          // its own label, never the sentence styling above: the styling is
          // the claim "the daemon said this", and nothing here was
          // verified. This quoting is a mitigation, not a fix: a hostile
          // child can still write instructions into the excerpt; the block
          // only keeps the reader able to tell whose words they are. The
          // text renders verbatim — never re-truncated, never un-escaped —
          // through React's default escaping, which keeps every byte inert.
          return (
            <figure className="workspace-chat-child-said">
              <figcaption>the child&apos;s own words</figcaption>
              <blockquote>{item.excerpt}</blockquote>
              {excerptRender.note === null ? null : (
                <p className="workspace-chat-child-said-note" role="note">
                  {excerptRender.note}
                </p>
              )}
            </figure>
          );
        })()}
      </div>
    );
  }

  if (item.role === "daemon_notice") {
    return <DaemonNoticeCard key={item.id} item={item} />;
  }

  if (item.role === "a2a_message") {
    return <A2aMessageCard key={item.id} item={item} names={a2aNames} />;
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
  daemonState,
  sessionRoster,
  deviceNames,
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
    journalLoss: null,
  });
  const conversationRef = useRef<HTMLDivElement>(null);
  // The name source the a2a card resolves against, rebuilt only when a roster
  // the workspace handed down changes: resolution happens at render, so a
  // rename or a re-pairing is visible the next time the card paints.
  const a2aNames = useMemo<A2aNameSource>(() => {
    const sessionById = new Map(
      (sessionRoster ?? []).map((session) => [session.id, session] as const),
    );
    return { sessionById, deviceNames: deviceNames ?? new Map<string, string>() };
  }, [sessionRoster, deviceNames]);

  useEffect(() => {
    const session = new AgentSession({
      sessionId,
      invoke: invokeAgentCommand,
      createChannel: createSessionChannel,
      onPermissionRequest: onPermissionRequest
        ? (request, subscriptionId) => onPermissionRequest(sessionId, subscriptionId, request)
        : undefined,
      onPermissionResolved: onPermissionResolved
        ? (resolution) => onPermissionResolved(sessionId, resolution)
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
  // The daemon connection is a global fact with its own channel. The gate
  // covers the two states where the supervisor has cleared the client, so
  // every send is guaranteed to fail: `disconnected` (ConnectionLost or
  // Stopped) and `connecting` (the top of each reconnect attempt, client
  // already gone). `error` and `unresponsive` are published while the client
  // is still installed — gating them would lock every composer on a single
  // failed ping. Keeping input live there is a judgement about likely
  // failure, not a guarantee: sends may be slow, and a failure is recorded
  // as a turn-level note.
  const daemonGone = daemonState === "disconnected" || daemonState === "connecting";
  const { copy: statusLabel, tone: statusDot } = toolbarStatus(observedState, elapsedMs, state);
  // `AgentSession` replaces the items array on every update (copy-on-write),
  // so this memo recomputes whenever the transcript changes and can never
  // go stale; it only skips work on re-renders with identical items.
  const entries = useMemo(() => groupToolCalls(state.items), [state.items]);
  const composerDisabled =
    osGone || daemonGone || (state.status !== "idle" && state.status !== "running");
  // The session's own terminal verdict outranks the transient daemon states:
  // a gone session must be named as gone even while a reconnect is pending.
  const sessionGone = osGone || state.status === "error" || state.status === "closed";
  const disabledReason = sessionGone
    ? "This session is no longer available."
    : daemonGone
      ? "The agent daemon is not connected."
      : state.status === "initializing"
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
        {entries.map((entry) => renderEntry(entry, a2aNames))}
        {state.streaming && !osGone ? (
          <div className="workspace-chat-typing" role="status">
            Agent is working
            <span className="workspace-stream-caret" aria-hidden="true" />
          </div>
        ) : null}
        {finishCopy !== null ? <div className="workspace-chat-finish">{finishCopy}</div> : null}
        {auxiliary}
      </div>
      {state.journalLoss !== null ? (
        <div
          className="workspace-journal-banner"
          role="status"
          data-testid="journal-degraded-banner"
        >
          {journalLossCopy(state.journalLoss)}
        </div>
      ) : null}
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
                disabled={composerDisabled}
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
                disabled={composerDisabled}
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
                disabled={composerDisabled}
              />
            ) : null}
          </>
        }
      />
    </div>
  );
});
