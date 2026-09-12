import type {
  ActiveTurnBehavior,
  PermissionRequest,
  PromptAttachment,
  SessionEvent,
  SessionManifest,
  ToolLocation,
} from "../types/ipc";
import type { SessionChannel } from "./tauri";

export type AgentChannel = SessionChannel;
export type AgentStatus = "initializing" | "idle" | "running" | "error" | "closed";
export type AgentSubagentStatus = "running" | "finished" | "failed" | "stopped" | "unknown";

export interface AgentSubagent {
  id: string;
  title: string | null;
  subagentType: string | null;
  status: AgentSubagentStatus;
  rawStatus: string | null;
  summary: string | null;
  parentToolUseId: string | null;
  spawnDepth: number | null;
  isBackground: boolean | null;
}

export type AgentSubagentStatusCounts = Record<AgentSubagentStatus, number>;

export type AgentChatItem =
  | {
      id: string;
      role: "user" | "assistant" | "thought";
      text: string;
      messageId: string | null;
      parentToolUseId?: string;
      spawnDepth?: number;
    }
  | {
      id: string;
      role: "tool";
      title: string;
      output: string;
      toolCallId: string;
      status: string;
      kind?: string;
      locations?: ToolLocation[];
      parentToolUseId?: string;
      spawnDepth?: number;
      subagentType?: string;
    }
  | { id: string; role: "error"; text: string }
  | { id: string; role: "system"; text: string; severity: "info" | "warning" };

export interface AgentFinished {
  stopReason: string;
  modelId?: string;
  usage?: Extract<SessionEvent, { type: "agent_finished" }>["usage"];
}

export interface AgentSessionState {
  items: AgentChatItem[];
  status: AgentStatus;
  streaming: boolean;
  availableCommands: Array<{ name: string; description: string; hint?: string }>;
  subagents: AgentSubagent[];
  subagentStatusCounts: AgentSubagentStatusCounts;
  lastFinished: AgentFinished | null;
  manifest: SessionManifest | null;
  /** A model/effort switch sent to the daemon that no manifest confirmed yet. */
  pendingSwitch: { modelId?: string; effort?: string; at: number } | null;
  /** A mode switch shown optimistically until the next manifest confirms it. */
  pendingModeId: string | null;
}

export interface AgentSessionDeps {
  sessionId: string;
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
  createChannel: (onEvent: (event: SessionEvent) => void) => AgentChannel;
  onPermissionRequest?: (request: PermissionRequest, subscriptionId: number) => void;
  onPermissionResolved?: (toolCallId: string) => void;
}

const INITIAL_STATE: AgentSessionState = {
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
};

const SWITCH_CONFIRM_TIMEOUT_MS = 15_000;

type MessageRole = "user" | "assistant" | "thought";

function eventError(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "object" && error !== null && "message" in error) {
    const message = error.message;
    if (typeof message === "string" && message.trim()) return message;
  }
  return "The agent session did not answer.";
}

function itemParentage(
  parentToolUseId?: string,
  spawnDepth?: number,
): {
  parentToolUseId?: string;
  spawnDepth?: number;
} {
  return {
    ...(parentToolUseId === undefined ? {} : { parentToolUseId }),
    ...(spawnDepth === undefined ? {} : { spawnDepth }),
  };
}

function normalizeTaskNotificationStatus(
  status: "completed" | "failed" | "stopped",
): AgentSubagentStatus {
  switch (status) {
    case "completed":
      return "finished";
    case "failed":
      return "failed";
    case "stopped":
      return "stopped";
  }
}

/**
 * Headless ACP session controller. The daemon owns the agent process; this
 * class only owns the attachment, prompt ordering, and derived chat view.
 */
export class AgentSession {
  private state: AgentSessionState = INITIAL_STATE;
  private readonly listeners = new Set<() => void>();
  private readonly blocks = new Map<string, number>();
  private readonly activeBlocks = new Map<string, string>();
  private activeRole: MessageRole | null = null;
  private nextItemId = 1;
  private nextAnonymousBlock = 1;
  private turn = 0;
  private started = false;
  private attached = false;
  private channel: AgentChannel | null = null;
  private readonly pendingPermissionRequests: PermissionRequest[] = [];
  private subscriptionId: number | null = null;
  private detachPromise: Promise<void> | null = null;
  private turnOpen = false;
  private disposed = false;
  private switchTimer: ReturnType<typeof setTimeout> | null = null;
  private modeTimer: ReturnType<typeof setTimeout> | null = null;
  private modeRequest = 0;

  constructor(private readonly deps: AgentSessionDeps) {}

  getState(): AgentSessionState {
    return this.state;
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  async start(): Promise<void> {
    if (this.started || this.disposed) return;
    this.started = true;
    const channel = this.deps.createChannel((event) => this.handleEvent(event));
    this.channel = channel;

    try {
      const subscriptionId = await this.deps.invoke<number>("session_attach", {
        id: this.deps.sessionId,
        fromCursor: null,
        ch: channel,
      });
      if (!Number.isSafeInteger(subscriptionId) || subscriptionId <= 0) {
        throw new Error("The daemon returned an invalid session subscription.");
      }
      this.subscriptionId = subscriptionId;
      this.attached = true;
      if (!this.disposed) {
        this.update({ status: "idle" });
        this.deliverPendingPermissionRequests();
      }
    } catch (error) {
      this.fail(`Could not attach the agent session: ${eventError(error)}`);
    }

    if (this.disposed && this.subscriptionId !== null) await this.detach();
  }

  getSubscriptionId(): number | null {
    return this.subscriptionId;
  }

  /**
   * Send one prompt. `activeTurnBehavior: "steer"` asks the daemon to deliver
   * this text into the turn that is already running instead of interrupting
   * it; omitted, the daemon keeps its interrupt-and-replace default. This is
   * the same path either way — steering is a property of the send, not a
   * second send method.
   *
   * A steer does not open a turn: the daemon writes the text into the turn in
   * flight and publishes the same `AgentUserMessage` echo every send produces
   * (`session.rs` publishes it for the whole send path, steer included), so the
   * transcript gains one inline user bubble inside the running turn. Bumping
   * the turn counter or appending a second bubble here would split the
   * assistant stream that is still arriving — the sender must not render the
   * steer twice, and Paseo emits its timeline item when the stream echoes the
   * steer, not at dispatch.
   */
  async send(
    text: string,
    attachments: readonly PromptAttachment[] = [],
    activeTurnBehavior?: ActiveTurnBehavior,
  ): Promise<boolean> {
    const trimmed = text.trim();
    if (!trimmed || this.disposed || !this.started || !this.attached) return false;
    if (this.state.status === "closed") return false;
    const subscriptionId = this.subscriptionId;
    if (subscriptionId === null) return false;

    // A steer only joins a turn when one is actually open; with no live turn
    // the daemon starts a new one, and so does the transcript.
    const joinsRunningTurn = activeTurnBehavior === "steer" && this.turnOpen;
    if (!joinsRunningTurn) this.beginTurn();
    this.update({ status: "running", streaming: true });
    try {
      await this.deps.invoke("session_send", {
        id: this.deps.sessionId,
        subscriptionId,
        text: trimmed,
        // Omitted when empty so a caller that attaches nothing produces exactly
        // the payload it produced before attachments existed. The daemon reads
        // an absent field as an empty list.
        ...(attachments.length === 0 ? {} : { attachments }),
        // Omitted, not `"interrupt"`, for a plain send: the daemon's own
        // default is interrupt-and-replace. Present only when the caller asked
        // for a send to join the turn that is already running.
        ...(activeTurnBehavior === undefined ? {} : { activeTurnBehavior }),
      });
      return true;
    } catch (error) {
      this.fail(`Could not send the message: ${eventError(error)}`);
      return false;
    }
  }

  /**
   * Interrupt the current turn without closing the session. The daemon
   * answers Ok even if the agent had no turn in flight; an error is
   * swallowed because a lost interrupt must not fail the chat view.
   */
  async interrupt(): Promise<void> {
    if (this.disposed || !this.started || !this.attached) return;
    const subscriptionId = this.subscriptionId;
    if (subscriptionId === null) return;
    try {
      await this.deps.invoke("session_interrupt", {
        id: this.deps.sessionId,
        subscriptionId,
      });
    } catch {
      // The turn keeps running; the status strip already reflects reality.
    }
  }

  /**
   * Hot-switch the model or its thinking effort within the fixed provider.
   * The invoke response is not a confirmation — the runtime confirms through
   * a later session_manifest event, so this only marks the switch as pending
   * and reports a rejected call through the chat error path, like send().
   */
  async setModel(modelId?: string, effort?: string): Promise<void> {
    if (this.disposed || !this.started || !this.attached) return;
    if (this.state.status === "closed") return;
    if (modelId === undefined && effort === undefined) return;

    this.update({ pendingSwitch: { modelId, effort, at: Date.now() } });
    try {
      await this.deps.invoke("session_set_model", {
        id: this.deps.sessionId,
        ...(modelId === undefined ? {} : { modelId }),
        ...(effort === undefined ? {} : { effort }),
      });
    } catch (error) {
      this.update({ pendingSwitch: null });
      this.fail(`Could not switch the model: ${eventError(error)}`);
      return;
    }
    if (this.disposed || this.state.pendingSwitch === null) {
      // A manifest that arrived during the invoke already confirmed or
      // superseded the switch; the pending marker is handled.
      return;
    }
    this.armSwitchTimeout();
  }

  /**
   * Hot-switch the permission mode. Like setModel, the invoke response is not
   * a confirmation: the chip shows the chosen mode optimistically until a
   * later session_manifest reports it, and a rejected invoke reverts to the
   * manifest value through the chat error path.
   */
  async setMode(modeId: string): Promise<void> {
    if (this.disposed || !this.started || !this.attached) return;
    if (this.state.status === "closed") return;
    if (!modeId) return;
    if (modeId === (this.state.pendingModeId ?? this.state.manifest?.modes?.currentModeId)) return;

    // A stale invoke result (a newer selection superseded it) must neither
    // revert the newer pending mode nor report its error, so each request
    // carries its generation.
    const requestId = ++this.modeRequest;
    this.update({ pendingModeId: modeId });
    try {
      await this.deps.invoke("session_set_mode", {
        id: this.deps.sessionId,
        modeId,
      });
    } catch (error) {
      if (requestId !== this.modeRequest) return;
      this.update({ pendingModeId: null });
      this.fail(`Could not switch the mode: ${eventError(error)}`);
      return;
    }
    if (this.disposed || this.state.pendingModeId === null || requestId !== this.modeRequest) {
      return;
    }
    this.armModeTimeout();
  }

  handleEvent(event: SessionEvent): void {
    if (this.disposed) return;

    switch (event.type) {
      case "agent_user_message":
        this.ensureTurn();
        this.closeActiveBlocks();
        this.appendText("user", event.messageId, event.text);
        return;
      case "agent_message":
        this.ensureTurn();
        this.appendText(
          "assistant",
          event.messageId,
          event.text,
          event.parentToolUseId,
          event.spawnDepth,
        );
        return;
      case "agent_thought":
        this.ensureTurn();
        this.appendText(
          "thought",
          event.messageId,
          event.text,
          event.parentToolUseId,
          event.spawnDepth,
        );
        return;
      case "session_notice":
        this.closeActiveBlocks();
        this.update({
          items: [
            ...this.state.items,
            {
              id: `system-${this.nextItemId++}`,
              role: "system",
              text: event.text,
              severity: event.severity,
            },
          ],
        });
        return;
      case "agent_finished":
        this.turnOpen = false;
        this.closeActiveBlocks();
        this.update({
          status: "idle",
          streaming: false,
          lastFinished: {
            stopReason: event.stopReason,
            ...(event.modelId === undefined ? {} : { modelId: event.modelId }),
            ...(event.usage === undefined ? {} : { usage: event.usage }),
          },
        });
        return;
      case "agent_task_started":
        this.startSubagent(event);
        return;
      case "agent_task_notification":
        this.notifySubagent(event);
        return;
      case "agent_background_tasks_changed":
        this.reconcileBackgroundTasks(event.tasks);
        return;
      case "agent_error":
        this.fail(event.message || "The agent reported an unknown error.");
        return;
      case "available_commands":
        this.update({ availableCommands: event.commands });
        return;
      case "permission_request":
        // The channel is live before session_attach confirms, so a request
        // can arrive while the subscription id is still unknown; hold it and
        // deliver it once the id exists instead of dropping it.
        if (this.subscriptionId === null) this.pendingPermissionRequests.push(event);
        else this.deps.onPermissionRequest?.(event, this.subscriptionId);
        return;
      case "permission_resolved":
        this.deps.onPermissionResolved?.(event.toolCallId);
        return;
      case "session_manifest": {
        // The provider also pushes spontaneous manifest updates (grok's
        // models/update); only a manifest that confirms or supersedes a
        // pending switch or mode resolves it. Everything else leaves the
        // strip dimmed and the backstop timer running.
        const previous = this.state.manifest;
        let pendingSwitch = this.state.pendingSwitch;
        let pendingModeId = this.state.pendingModeId;
        if (pendingSwitch !== null && this.manifestResolvesSwitch(event, previous, pendingSwitch)) {
          pendingSwitch = null;
        }
        if (pendingModeId !== null && this.manifestResolvesMode(event, previous, pendingModeId)) {
          pendingModeId = null;
        }
        if (pendingSwitch === null) this.clearSwitchTimer();
        if (pendingModeId === null) this.clearModeTimer();
        this.update({ manifest: event, pendingSwitch, pendingModeId });
        return;
      }
      case "agent_tool_call":
        this.ensureTurn();
        this.appendTool(
          event.toolCallId,
          event.title,
          event.status,
          event.parentToolUseId,
          event.spawnDepth,
          event.subagentType,
          event.kind,
          event.locations,
        );
        return;
      case "agent_tool_update":
        this.ensureTurn();
        this.updateTool(
          event.toolCallId,
          event.status,
          event.text,
          event.parentToolUseId,
          event.spawnDepth,
          event.kind,
          event.locations,
          event.title,
        );
        return;
      case "exit":
        this.stopRunningSubagents();
        if (this.turnOpen) {
          this.fail("The agent stopped before finishing this turn.");
        } else {
          this.update({ status: "closed", streaming: false });
        }
        return;
      case "recovered":
        this.stopRunningSubagents();
        this.fail("This agent session is no longer available.");
        return;
      case "output":
      case "agent_stderr":
      case "silent":
      case "journal_degraded":
      case "sessions_snapshot":
      case "snapshot":
      case "agent_reported":
      // Journaled for audit and not emitted to observers; the transcript gains
      // steer rendering in slice 4b.
      case "steered":
        return;
    }
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.clearSwitchTimer();
    this.clearModeTimer();
    this.pendingPermissionRequests.length = 0;
    if (this.channel !== null) {
      this.channel.onmessage = () => undefined;
      this.channel = null;
    }
    if (this.subscriptionId !== null) void this.detach();
    this.listeners.clear();
  }

  /**
   * Whether a session_manifest resolves the pending switch: it either
   * confirms the requested model/effort, or a third party moved the model
   * away (superseded — the request can no longer be honored).
   */
  private manifestResolvesSwitch(
    event: SessionManifest,
    previous: SessionManifest | null,
    pending: { modelId?: string; effort?: string; at: number },
  ): boolean {
    if (pending.modelId !== undefined) {
      if (event.currentModelId === pending.modelId) return true;
      return (
        previous?.currentModelId !== undefined &&
        event.currentModelId !== undefined &&
        event.currentModelId !== previous.currentModelId
      );
    }
    const current = event.models.find((model) => model.modelId === event.currentModelId);
    if (current?.currentEffort === pending.effort) return true;
    return (
      previous?.currentModelId !== undefined &&
      event.currentModelId !== undefined &&
      event.currentModelId !== previous.currentModelId
    );
  }

  /**
   * Whether a session_manifest resolves the pending mode: it either reports
   * the requested mode, or a third party moved the mode away (superseded).
   */
  private manifestResolvesMode(
    event: SessionManifest,
    previous: SessionManifest | null,
    pendingModeId: string,
  ): boolean {
    const current = event.modes?.currentModeId;
    if (current === pendingModeId) return true;
    return (
      previous?.modes !== undefined &&
      current !== undefined &&
      current !== previous.modes.currentModeId
    );
  }

  /** Deliver the requests held while the subscription id was still unknown, exactly once. */
  private deliverPendingPermissionRequests(): void {
    if (this.disposed || this.subscriptionId === null) return;
    const requests = this.pendingPermissionRequests.splice(0);
    const subscriptionId = this.subscriptionId;
    for (const request of requests) this.deps.onPermissionRequest?.(request, subscriptionId);
  }

  private clearSwitchTimer(): void {
    if (this.switchTimer !== null) {
      clearTimeout(this.switchTimer);
      this.switchTimer = null;
    }
  }

  private clearModeTimer(): void {
    if (this.modeTimer !== null) {
      clearTimeout(this.modeTimer);
      this.modeTimer = null;
    }
  }

  private armSwitchTimeout(): void {
    this.clearSwitchTimer();
    this.switchTimer = setTimeout(() => {
      this.switchTimer = null;
      if (!this.disposed) this.update({ pendingSwitch: null });
    }, SWITCH_CONFIRM_TIMEOUT_MS);
  }

  private armModeTimeout(): void {
    this.clearModeTimer();
    this.modeTimer = setTimeout(() => {
      this.modeTimer = null;
      if (!this.disposed) this.update({ pendingModeId: null });
    }, SWITCH_CONFIRM_TIMEOUT_MS);
  }

  detach(): Promise<void> {
    if (this.detachPromise !== null) return this.detachPromise;
    const subscriptionId = this.subscriptionId;
    if (subscriptionId === null) return Promise.resolve();
    // Clear before IPC so an overlapping replacement can never detach this new view.
    this.subscriptionId = null;
    this.attached = false;
    this.detachPromise = this.deps
      .invoke("session_detach", { subscriptionId })
      .then(() => undefined)
      .catch(() => undefined);
    return this.detachPromise;
  }

  private beginTurn(): void {
    this.turn += 1;
    this.turnOpen = true;
    this.closeActiveBlocks();
    this.update({ lastFinished: null });
  }

  private ensureTurn(): void {
    if (!this.turnOpen) this.beginTurn();
  }

  private appendText(
    role: MessageRole,
    messageId: string | null,
    text: string,
    parentToolUseId?: string,
    spawnDepth?: number,
  ): void {
    this.prepareRole(role);
    const key = this.blockKey(role, messageId, parentToolUseId);
    const index = this.blocks.get(key);
    if (index === undefined) {
      const item: AgentChatItem = {
        id: `${role}-${this.nextItemId++}`,
        role,
        text,
        messageId,
        ...itemParentage(parentToolUseId, spawnDepth),
      };
      this.blocks.set(key, this.state.items.length);
      this.activeBlocks.set(this.activeBlockKey(role, parentToolUseId), key);
      this.update({ items: [...this.state.items, item] });
      return;
    }

    const items = [...this.state.items];
    const item = items[index];
    if (item.role !== role) return;
    items[index] = { ...item, text: item.text + text };
    this.activeBlocks.set(this.activeBlockKey(role, parentToolUseId), key);
    this.update({ items });
  }

  private appendTool(
    toolCallId: string,
    title: string,
    status: string,
    parentToolUseId?: string,
    spawnDepth?: number,
    subagentType?: string,
    kind?: string,
    locations?: ToolLocation[],
    output = "",
  ): void {
    // A tool-call item is a transcript boundary. Tool updates for an existing
    // item mutate it in place and must not close text that arrived afterward.
    this.closeActiveBlocks();
    const key = `tool:${this.turn}:${toolCallId}`;
    const index = this.blocks.get(key);
    if (index === undefined) {
      this.blocks.set(key, this.state.items.length);
      this.update({
        items: [
          ...this.state.items,
          {
            id: `tool-${this.nextItemId++}`,
            role: "tool",
            title,
            output,
            toolCallId,
            status,
            ...(kind === undefined ? {} : { kind }),
            ...(locations === undefined ? {} : { locations }),
            ...itemParentage(parentToolUseId, spawnDepth),
            ...(subagentType === undefined ? {} : { subagentType }),
          },
        ],
      });
      return;
    }

    const item = this.state.items[index];
    if (item.role !== "tool") return;
    const items = [...this.state.items];
    items[index] = { ...item, status };
    this.update({ items });
  }

  private updateTool(
    toolCallId: string,
    status: string | null,
    text: string | null,
    parentToolUseId?: string,
    spawnDepth?: number,
    kind?: string,
    locations?: ToolLocation[],
    title?: string,
  ): void {
    const key = `tool:${this.turn}:${toolCallId}`;
    const index = this.blocks.get(key);
    const nextTitle = typeof title === "string" && title.length > 0 ? title : undefined;
    if (index === undefined) {
      this.appendTool(
        toolCallId,
        nextTitle ?? text ?? "Tool call",
        status ?? "running",
        parentToolUseId,
        spawnDepth,
        undefined,
        kind,
        locations,
        nextTitle === undefined ? "" : (text ?? ""),
      );
      return;
    }

    const item = this.state.items[index];
    if (item.role !== "tool") return;
    const items = [...this.state.items];
    items[index] = {
      ...item,
      status: status ?? item.status,
      ...(text === null || text === ""
        ? {}
        : { output: item.output ? `${item.output}\n${text}` : text }),
      ...(nextTitle === undefined ? {} : { title: nextTitle }),
      ...(kind === undefined ? {} : { kind }),
      ...(locations === undefined ? {} : { locations }),
    };
    this.update({ items });
  }

  private startSubagent(event: Extract<SessionEvent, { type: "agent_task_started" }>): void {
    const current = this.state.subagents.find((subagent) => subagent.id === event.taskId);
    this.replaceSubagent({
      id: event.taskId,
      title: event.title ?? current?.title ?? null,
      subagentType: event.subagentType ?? current?.subagentType ?? null,
      status: "running",
      rawStatus: null,
      summary: null,
      parentToolUseId: event.toolUseId ?? current?.parentToolUseId ?? null,
      spawnDepth: event.spawnDepth ?? current?.spawnDepth ?? null,
      isBackground: event.isBackgrounded ?? current?.isBackground ?? null,
    });
  }

  private notifySubagent(event: Extract<SessionEvent, { type: "agent_task_notification" }>): void {
    const current = this.state.subagents.find((subagent) => subagent.id === event.taskId);
    this.replaceSubagent({
      id: event.taskId,
      title: current?.title ?? null,
      subagentType: current?.subagentType ?? null,
      status: normalizeTaskNotificationStatus(event.status),
      rawStatus: event.status,
      summary: event.summary ?? null,
      parentToolUseId: event.toolUseId ?? current?.parentToolUseId ?? null,
      spawnDepth: current?.spawnDepth ?? null,
      isBackground: current?.isBackground ?? null,
    });
  }

  private reconcileBackgroundTasks(
    tasks: Extract<SessionEvent, { type: "agent_background_tasks_changed" }>["tasks"],
  ): void {
    const background = new Set(tasks.map((task) => task.taskId));
    const known = new Set(this.state.subagents.map((subagent) => subagent.id));
    const subagents = this.state.subagents.map((subagent) => ({
      ...subagent,
      isBackground: background.has(subagent.id),
    }));
    for (const task of tasks) {
      if (known.has(task.taskId)) continue;
      subagents.push({
        id: task.taskId,
        title: task.title,
        subagentType: null,
        status: "unknown",
        rawStatus: null,
        summary: null,
        parentToolUseId: null,
        spawnDepth: null,
        isBackground: true,
      });
    }
    this.updateSubagents(subagents);
  }

  private replaceSubagent(next: AgentSubagent): void {
    const current = this.state.subagents.some((subagent) => subagent.id === next.id);
    const subagents = current
      ? this.state.subagents.map((subagent) => (subagent.id === next.id ? next : subagent))
      : [...this.state.subagents, next];
    this.updateSubagents(subagents);
  }

  private stopRunningSubagents(): void {
    // A closed parent cannot keep a child running. This also settles an
    // unknown child created only by a background membership snapshot: it has
    // no lifecycle bookend, but must not leave a stale indicator.
    const subagents = this.state.subagents.map((subagent) =>
      subagent.status === "running" || subagent.status === "unknown"
        ? { ...subagent, status: "stopped" as const }
        : subagent,
    );
    if (subagents.some((subagent, index) => subagent !== this.state.subagents[index])) {
      this.updateSubagents(subagents);
    }
  }

  private updateSubagents(subagents: AgentSubagent[]): void {
    const counts: AgentSubagentStatusCounts = {
      running: 0,
      finished: 0,
      failed: 0,
      stopped: 0,
      unknown: 0,
    };
    for (const subagent of subagents) counts[subagent.status] += 1;
    this.update({ subagents, subagentStatusCounts: counts });
  }

  private blockKey(role: MessageRole, messageId: string | null, parentToolUseId?: string): string {
    const parent = parentToolUseId ?? "";
    if (messageId !== null) return `${role}:${this.turn}:${parent}:${messageId}`;
    const active = this.activeBlocks.get(this.activeBlockKey(role, parentToolUseId));
    if (active !== undefined) return active;
    return `${role}:${this.turn}:${parent}:anonymous:${this.nextAnonymousBlock++}`;
  }

  private activeBlockKey(role: MessageRole, parentToolUseId?: string): string {
    return `${role}:${parentToolUseId ?? ""}`;
  }

  /**
   * An id-less stream is one open block only while its role stays active.
   * Explicit message ids can still reactivate their keyed block after an
   * interleaving role, preserving the existing keyed replay behavior.
   */
  private prepareRole(role: MessageRole): void {
    if (this.activeRole === role) return;
    this.closeActiveBlocks();
    this.activeRole = role;
  }

  private closeActiveBlocks(): void {
    this.activeBlocks.clear();
    this.activeRole = null;
  }

  private fail(message: string): void {
    this.turnOpen = false;
    this.closeActiveBlocks();
    this.update({
      status: "error",
      streaming: false,
      items: [
        ...this.state.items,
        { id: `error-${this.nextItemId++}`, role: "error", text: message },
      ],
    });
  }

  private update(patch: Partial<AgentSessionState>): void {
    this.state = { ...this.state, ...patch };
    // A listener may dispose or unsubscribe during notification; a snapshot prevents that
    // mutation from skipping listeners that were already subscribed for this update.
    for (const listener of [...this.listeners]) listener();
  }
}
