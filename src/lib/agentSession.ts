import type { Channel } from "@tauri-apps/api/core";
import type { PermissionRequest, SessionEvent, SessionManifest } from "../types/ipc";

export type AgentChannel = Channel<SessionEvent>;
export type AgentStatus = "initializing" | "idle" | "running" | "error" | "closed";

export type AgentChatItem =
  | { id: string; role: "user" | "assistant" | "thought"; text: string; messageId: string | null }
  | { id: string; role: "tool"; text: string; toolCallId: string; status: string }
  | { id: string; role: "error"; text: string };

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
  lastFinished: AgentFinished | null;
  manifest: SessionManifest | null;
  /** A model/effort switch sent to the daemon that no manifest confirmed yet. */
  pendingSwitch: { modelId?: string; effort?: string; at: number } | null;
}

export interface AgentSessionDeps {
  sessionId: string;
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
  createChannel: (onEvent: (event: SessionEvent) => void) => AgentChannel;
  onPermissionRequest?: (request: PermissionRequest) => void;
  onPermissionResolved?: (toolCallId: string) => void;
  isSuperseded?: () => boolean;
}

const INITIAL_STATE: AgentSessionState = {
  items: [],
  status: "initializing",
  streaming: false,
  availableCommands: [],
  lastFinished: null,
  manifest: null,
  pendingSwitch: null,
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

/**
 * Headless ACP session controller. The daemon owns the agent process; this
 * class only owns the attachment, prompt ordering, and derived chat view.
 */
export class AgentSession {
  private state: AgentSessionState = INITIAL_STATE;
  private readonly listeners = new Set<() => void>();
  private readonly blocks = new Map<string, number>();
  private readonly activeBlocks = new Map<MessageRole, string>();
  private nextItemId = 1;
  private turn = 0;
  private started = false;
  private attached = false;
  private turnOpen = false;
  private disposed = false;
  private switchTimer: ReturnType<typeof setTimeout> | null = null;

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

    try {
      await this.deps.invoke("session_attach", {
        id: this.deps.sessionId,
        fromCursor: null,
        ch: channel,
      });
      this.attached = true;
      if (!this.disposed) this.update({ status: "idle" });
    } catch (error) {
      this.fail(`Could not attach the agent session: ${eventError(error)}`);
    }

    if (this.disposed && this.attached && !this.deps.isSuperseded?.()) await this.detach();
  }

  async send(text: string): Promise<boolean> {
    const trimmed = text.trim();
    if (!trimmed || this.disposed || !this.started || !this.attached) return false;
    if (this.state.status === "closed") return false;

    this.beginTurn();
    this.update({ status: "running", streaming: true });
    try {
      await this.deps.invoke("session_send", { id: this.deps.sessionId, text: trimmed });
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
    try {
      await this.deps.invoke("session_interrupt", { id: this.deps.sessionId });
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

  handleEvent(event: SessionEvent): void {
    if (this.disposed) return;

    switch (event.type) {
      case "agent_user_message":
        this.ensureTurn();
        this.appendText("user", event.messageId, event.text);
        return;
      case "agent_message":
        this.ensureTurn();
        this.appendText("assistant", event.messageId, event.text);
        return;
      case "agent_thought":
        this.ensureTurn();
        this.appendText("thought", event.messageId, event.text);
        return;
      case "agent_finished":
        this.turnOpen = false;
        this.activeBlocks.clear();
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
      case "agent_error":
        this.fail(event.message || "The agent reported an unknown error.");
        return;
      case "available_commands":
        this.update({ availableCommands: event.commands });
        return;
      case "permission_request":
        this.deps.onPermissionRequest?.(event);
        return;
      case "permission_resolved":
        this.deps.onPermissionResolved?.(event.toolCallId);
        return;
      case "session_manifest": {
        // The provider also pushes spontaneous manifest updates (grok's
        // models/update); only a manifest that confirms or supersedes the
        // pending switch resolves it. Everything else leaves the strip dimmed
        // and the backstop timer running.
        const pending = this.state.pendingSwitch;
        const previous = this.state.manifest;
        if (pending !== null && this.manifestResolvesSwitch(event, previous, pending)) {
          this.clearSwitchTimer();
          this.update({ manifest: event, pendingSwitch: null });
        } else {
          this.update({ manifest: event });
        }
        return;
      }
      case "agent_tool_call":
        this.ensureTurn();
        this.appendTool(event.toolCallId, event.title, event.status);
        return;
      case "agent_tool_update":
        this.ensureTurn();
        this.updateTool(event.toolCallId, event.status, event.text);
        return;
      case "exit":
        if (this.turnOpen) {
          this.fail("The agent stopped before finishing this turn.");
        } else {
          this.update({ status: "closed", streaming: false });
        }
        return;
      case "recovered":
        this.fail("This agent session is no longer available.");
        return;
      case "output":
      case "agent_stderr":
      case "silent":
      case "journal_degraded":
      case "sessions_snapshot":
      case "snapshot":
      case "agent_reported":
        return;
    }
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.clearSwitchTimer();
    if (this.attached) void this.detach();
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

  private clearSwitchTimer(): void {
    if (this.switchTimer !== null) {
      clearTimeout(this.switchTimer);
      this.switchTimer = null;
    }
  }

  private armSwitchTimeout(): void {
    this.clearSwitchTimer();
    this.switchTimer = setTimeout(() => {
      this.switchTimer = null;
      if (!this.disposed) this.update({ pendingSwitch: null });
    }, SWITCH_CONFIRM_TIMEOUT_MS);
  }

  private async detach(): Promise<void> {
    if (!this.attached) return;
    this.attached = false;
    try {
      await this.deps.invoke("session_detach", { id: this.deps.sessionId });
    } catch {
      // The view is already gone; a lost detach cannot make the chat useful.
    }
  }

  private beginTurn(): void {
    this.turn += 1;
    this.turnOpen = true;
    this.activeBlocks.clear();
    this.update({ lastFinished: null });
  }

  private ensureTurn(): void {
    if (!this.turnOpen) this.beginTurn();
  }

  private appendText(role: MessageRole, messageId: string | null, text: string): void {
    const key = this.blockKey(role, messageId);
    const index = this.blocks.get(key);
    if (index === undefined) {
      const item: AgentChatItem = {
        id: `${role}-${this.nextItemId++}`,
        role,
        text,
        messageId,
      };
      this.blocks.set(key, this.state.items.length);
      this.activeBlocks.set(role, key);
      this.update({ items: [...this.state.items, item] });
      return;
    }

    const items = [...this.state.items];
    const item = items[index];
    if (item.role !== role) return;
    items[index] = { ...item, text: item.text + text };
    this.activeBlocks.set(role, key);
    this.update({ items });
  }

  private appendTool(toolCallId: string, title: string, status: string): void {
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
            text: title,
            toolCallId,
            status,
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

  private updateTool(toolCallId: string, status: string | null, text: string | null): void {
    const key = `tool:${this.turn}:${toolCallId}`;
    const index = this.blocks.get(key);
    if (index === undefined) {
      this.appendTool(toolCallId, text ?? "Tool call", status ?? "running");
      return;
    }

    const item = this.state.items[index];
    if (item.role !== "tool") return;
    const items = [...this.state.items];
    items[index] = {
      ...item,
      status: status ?? item.status,
      ...(text === null ? {} : { text: `${item.text}\n${text}` }),
    };
    this.update({ items });
  }

  private blockKey(role: MessageRole, messageId: string | null): string {
    if (messageId !== null) return `${role}:${this.turn}:${messageId}`;
    const active = this.activeBlocks.get(role);
    if (active !== undefined) return active;
    return `${role}:${this.turn}`;
  }

  private fail(message: string): void {
    this.turnOpen = false;
    this.activeBlocks.clear();
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
    for (const listener of this.listeners) listener();
  }
}
