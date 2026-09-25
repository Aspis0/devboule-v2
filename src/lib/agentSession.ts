import type {
  ActiveTurnBehavior,
  ContextUsage,
  ErrorCode,
  PermissionRequest,
  PermissionResolved,
  PromptAttachment,
  SessionEvent,
  SessionManifest,
  ToolLocation,
} from "../types/ipc";
import { recordChildFinishedHistory } from "../features/design/childFinishedHistory";
import { scheduleDelegatedDesignMirror } from "../features/design/delegatedDesignMirror";
import type { AttachmentReference, SessionChannel } from "./tauri";
import { isCommandError } from "./commandError";
import { errorSentence } from "./errorSentence";
import { eventTypeName } from "./eventTypeName";
import { parseAgentPermissionRequest } from "./agentPermissionRequest";
import { parseAgentDaemonNotice, type AgentDaemonNotice } from "./agentDaemonNotice";
import { parseAgentPeerMessage, type AgentPeerOrigin } from "./agentPeerMessage";
import { recordPlanUsage } from "./planUsageStore";

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
  | {
      id: string;
      role: "error";
      text: string;
      /** The demoted raw text (env vars, OS errors, internal words), under the sentence. */
      detail?: string;
    }
  | { id: string; role: "system"; text: string; severity: "info" | "warning" }
  | {
      /** One `agent_permission_request` envelope, parsed (see `agentPermissionRequest.ts`). */
      id: string;
      role: "permission_request";
      cardId: string;
      toolTitle: string;
      childName: string;
      /** The child's own words, verbatim. Never re-truncated, never un-escaped. */
      excerpt: string;
      /**
       * Whether the excerpt's closing fence arrived: `"closed"`, or
       * `"unterminated"` — the opener came but the closer never did, so the
       * block ran to the envelope's end, which the chat surface renders as
       * its own visible note. `"absent"` means there was no excerpt block.
       */
      excerptState: "closed" | "unterminated" | "absent";
    }
  | {
      /** One daemon notice envelope, parsed (see `agentDaemonNotice.ts`): a
          known kind's facts, or an unrecognized frame kept visible. */
      id: string;
      role: "daemon_notice";
      notice: AgentDaemonNotice;
    }
  | {
      /** One agent-to-agent relay envelope, parsed (see `agentPeerMessage.ts`):
          another agent's message, named and stripped of the envelope. */
      id: string;
      role: "a2a_message";
      /** The sender the daemon's fixed header names. */
      fromAgent: string;
      /** What the frame's origin line commits to (`origin_line`): this
          machine, a paired device and its name when it names one, or
          nothing. */
      origin: AgentPeerOrigin;
      /** The sender's message, verbatim — hostile input, rendered as text only. */
      body: string;
    }
  | {
      /** This session's raw echo of a message it sent to another agent. */
      id: string;
      role: "a2a_outgoing_message";
      text: string;
    };

/**
 * The text of the last assistant message in a transcript, or null when the
 * transcript holds none. Only assistant prose counts: a thought, a tool row
 * or a relayed message is not what the agent said to the user.
 */
export function lastAssistantMessage(items: readonly AgentChatItem[]): string | null {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (item.role === "assistant") return item.text;
  }
  return null;
}

export interface AgentFinished {
  stopReason: string;
  modelId?: string;
  usage?: Extract<SessionEvent, { type: "agent_finished" }>["usage"];
}

export interface AgentSessionState {
  items: AgentChatItem[];
  /**
   * Readonly so the only possible write is through `setStatus`, which owns
   * the latch; `Partial<AgentSessionState>` keeps the modifier, so no patch
   * type can smuggle a write around it.
   */
  readonly status: AgentStatus;
  streaming: boolean;
  availableCommands: Array<{ name: string; description: string; hint?: string }>;
  subagents: AgentSubagent[];
  subagentStatusCounts: AgentSubagentStatusCounts;
  lastFinished: AgentFinished | null;
  /**
   * The latest context reading this session's stream delivered. Kept across
   * turns — `live: false` labels it "as of the last turn" instead of hiding
   * it — and cleared only with the session itself.
   */
  contextUsage: ContextUsage | null;
  manifest: SessionManifest | null;
  /** A model/effort switch sent to the daemon that no manifest confirmed yet. */
  pendingSwitch: { modelId?: string; effort?: string; at: number } | null;
  /** A mode switch shown optimistically until the next manifest confirms it. */
  pendingModeId: string | null;
  /**
   * Journal writes are failing for this session: the transcript on disk is
   * missing at least the frames counted here. Worst-known totals, never
   * cleared — what the journal dropped does not come back.
   */
  journalLoss: { frames: number; bytes: number } | null;
}

/**
 * A state patch that cannot carry `status`: the field is omitted from the
 * type and forbidden as `never`, so both `update({ status })` and
 * `update(wholeState)` fail to compile. Status moves only through
 * `setStatus`, which owns the latch.
 */
type AgentSessionPatch = Omit<Partial<AgentSessionState>, "status"> & { status?: never };

export interface AgentSessionDeps {
  sessionId: string;
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
  createChannel: (onEvent: (event: SessionEvent) => void) => AgentChannel;
  /**
   * Where a created child's finish is recorded. Absent means the Design history
   * (`recordChildFinishedHistory`), which is what every host but the read-only
   * history reopen wants. It is a dependency so that the one controller that
   * must not write anything can say so here instead of having its write refused
   * later: `designHistoryOpen` passes a recorder that records nothing.
   */
  onChildFinished?: (event: Extract<SessionEvent, { type: "child_finished" }>) => Promise<boolean>;
  onPermissionRequest?: (request: PermissionRequest, subscriptionId: number) => void;
  /**
   * One card resolved, with everything the wire said about it: who answered
   * (`answeredBy` — a session id for delegated answering, null/absent for a
   * person) and what was chosen. The field travels to a pixel through the
   * host, never inside this controller: a callback that took only the
   * `toolCallId` threw away exactly the fields the card's attribution needs,
   * which is the half-a-wiring shape this signature exists to prevent.
   */
  onPermissionResolved?: (resolution: PermissionResolved) => void;
}

const INITIAL_STATE: AgentSessionState = {
  items: [],
  status: "initializing",
  streaming: false,
  availableCommands: [],
  subagents: [],
  subagentStatusCounts: { running: 0, finished: 0, failed: 0, stopped: 0, unknown: 0 },
  lastFinished: null,
  contextUsage: null,
  manifest: null,
  pendingSwitch: null,
  pendingModeId: null,
  journalLoss: null,
};

const SWITCH_CONFIRM_TIMEOUT_MS = 15_000;

/**
 * Cap on remembered send-rejection texts awaiting their twin frame. See
 * `noteSendRejection` for when an entry expires.
 */
const MAX_PENDING_SEND_REJECTIONS = 8;

/**
 * What the `recovered` event records (see `handleEvent`): this view's attach
 * state, not a line of the transcript's history — a Reopen's fresh attach does
 * not replay it. Exported so the chat surface can drop this entry while the
 * workspace's reopen bar states the same fact once.
 */
export const RECOVERED_SESSION_UNAVAILABLE = "This agent session is no longer available.";

type MessageRole = "user" | "assistant" | "thought";

/**
 * Send-refusal codes that mean *our view of the session is gone*: whatever
 * happened on the far side, no event about this session will reach us again,
 * because events travel through the attachment we lost. The status must
 * latch — nothing can correct it later. Per entry, the producer:
 *
 * - `session_not_found` — the daemon holds no such session (unknown or
 *   deleted id); raised by the session commands.
 * - `session_generation_mismatch` — the session's generation moved on
 *   without us. No send raises it today, but it names exactly the gone-view
 *   case the daemon should be using; kept fatal on our side for when it does.
 * - `shutting_down` — the daemon is going away; the connection and every
 *   view on it go with it.
 * - `protocol_version_mismatch` — the bridge and the daemon cannot speak to
 *   each other; the view cannot be delivered through the connection.
 * - `connection_lost` — the daemon transport ended while this send was in
 *   flight; the view cannot speak for the session until it is reattached.
 *
 * Deliberately absent: `io` (carries DaemonError::TimedOut — a send that
 * timed out after the client's 30s may still have been delivered) and
 * `unauthorized` (a refused steer on a paired device — a live-session
 * refusal). `invalid_request` is ambiguous at the daemon today: process-gone
 * and observer-detachment ride it alongside genuine validity refusals, so it
 * stays turn-level here while the daemon is given codes that name a gone
 * view; those belong in this set when they land.
 */
const FATAL_SEND_CODES: ReadonlySet<ErrorCode> = new Set([
  "session_not_found",
  "session_generation_mismatch",
  "shutting_down",
  "protocol_version_mismatch",
  "connection_lost",
]);

function sendFailureKillsSession(error: unknown): boolean {
  return isCommandError(error) && FATAL_SEND_CODES.has(error.code);
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
  /** Context readings have their own lane (see `subscribeUsage`). */
  private readonly usageListeners = new Set<() => void>();
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
  /** How many `session_send` calls are awaiting the daemon (see `holdAgentError`). */
  private sendDepth = 0;
  private readonly heldAgentErrors: string[] = [];
  /**
   * Raw texts of send rejections whose `agent_error` twin may still be on the
   * wire. The daemon writes each send's reply before that send's frame (the
   * reply at the end of the send's own iteration, the published frame in a
   * later one — server/connection.rs), so with two sends in flight the wire
   * order `[A reply][A frame][B reply]` leaves A's twin pending after B has
   * already settled; one slot cannot hold two same-text twins either
   * (review-E1-fix2, P2-1 and P2-2). Entries are consumed one per frame by
   * exact text match; a frame that matches nothing is shown verbatim.
   */
  private readonly pendingSendRejections: string[] = [];
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

  /**
   * Subscribe to the session's context readings only. The transcript surface
   * re-renders on `subscribe`; the meter re-renders on this — a chatty
   * provider (five `thread/tokenUsage/updated` frames in the Codex capture's
   * twelve seconds) must not re-map the whole transcript per frame.
   */
  subscribeUsage(listener: () => void): () => void {
    this.usageListeners.add(listener);
    return () => {
      this.usageListeners.delete(listener);
    };
  }

  getContextUsage(): ContextUsage | null {
    return this.state.contextUsage;
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
        this.setStatus("idle");
        this.deliverPendingPermissionRequests();
      }
    } catch (error) {
      const mapped = errorSentence(error);
      this.failSession(
        `Could not attach the agent session. ${mapped.sentence}`,
        mapped.detail ?? undefined,
      );
    }

    if (this.disposed && this.subscriptionId !== null) await this.detach();
  }

  getSubscriptionId(): number | null {
    return this.subscriptionId;
  }

  /**
   * Send one prompt. `activeTurnBehavior: "steer"` asks the daemon to deliver
   * this text into the turn that is already running; **omitted, the send starts
   * a fresh turn and interrupts nothing** — the plain path writes the prompt to
   * the session and the only interrupt the daemon performs on it is the
   * steer-refusal fallback (`session_messaging.rs`, the `Steer` branch). A
   * caller that means to replace a running turn must interrupt it first.
   *
   * `idempotencyKey` is that key a retry needs: the same text sent again with
   * the same key is answered from the daemon's receipt instead of running a
   * second prompt. A plain composer send has no retry identity and sends none.
   *
   * A steer does not open a turn: the daemon writes the text into the turn in
   * flight and publishes the same `AgentUserMessage` echo every send produces
   * (`publish_agent_user_message` publishes it for the whole send path, steer included), so the
   * transcript gains one inline user bubble inside the running turn. Bumping
   * the turn counter or appending a second bubble here would split the
   * assistant stream that is still arriving — the sender must not render the
   * steer twice, and Paseo emits its timeline item when the stream echoes the
   * steer, not at dispatch.
   *
   * `attachmentReferences` names attachments that already travelled: one entry
   * per deposited page, in the order the pages appear in the composer. The
   * composer deposits a document's pages first (`transportDesignAttachments`) and
   * sends the references it was answered with, so a prompt carries a deck by
   * name rather than by bytes.
   */
  async send(
    text: string,
    attachments: readonly PromptAttachment[] = [],
    activeTurnBehavior?: ActiveTurnBehavior,
    attachmentReferences: readonly AttachmentReference[] = [],
    idempotencyKey?: string,
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
    this.setStatus("running", { streaming: true });
    this.sendDepth += 1;
    try {
      await this.deps.invoke("session_send", {
        id: this.deps.sessionId,
        subscriptionId,
        text: trimmed,
        // Omitted when empty so a caller that attaches nothing produces exactly
        // the payload it produced before attachments existed. The daemon reads
        // an absent field as an empty list.
        ...(attachments.length === 0 ? {} : { attachments }),
        // Absent for a plain send: the daemon starts a turn with it and
        // interrupts nothing. Present only when the caller asked the text to
        // join the turn that is already running.
        ...(activeTurnBehavior === undefined ? {} : { activeTurnBehavior }),
        // Absent, never empty: a send with no retry identity is every send that
        // predates the queue, and its frame must not grow a key.
        ...(idempotencyKey === undefined ? {} : { idempotencyKey }),
        // Omitted, not empty, when the prompt names no stored attachment: a send
        // with no deposit behind it produces exactly the payload it produced
        // before deposits existed, and the daemon reads an absent field as an
        // empty list.
        ...(attachmentReferences.length === 0 ? {} : { attachmentReferences }),
      });
      this.sendDepth -= 1;
      // A settled send clears nothing: a rejection earlier on the wire may
      // still be waiting for its twin frame (see `pendingSendRejections`).
      this.resolveHeldAgentErrors();
      return true;
    } catch (error) {
      this.sendDepth -= 1;
      // The daemon publishes agent_error with the same message it rejects
      // with, but the rejection is written to the connection synchronously
      // (server/connection.rs:311) while the event waits in the attachment
      // queue — the rejection arrives FIRST. Remember the raw text so the
      // late frame is dropped once below; held frames are resolved against
      // the pending list, exact matches only.
      const mapped = errorSentence(error);
      this.noteSendRejection(mapped.detail ?? mapped.sentence);
      this.resolveHeldAgentErrors();
      // The daemon names its failures: a capability or validity refusal is
      // raised inside a live send path and only codes naming a gone view end
      // the session (see `FATAL_SEND_CODES`). A refused steer was joining a
      // turn the daemon is still running — record the sentence and leave the
      // turn alone; ending it would split the answer when the chunks resume.
      const detail = `Could not send the message. ${mapped.sentence}`;
      if (sendFailureKillsSession(error)) this.failSession(detail, mapped.detail ?? undefined);
      else if (joinsRunningTurn) this.noteError(detail, mapped.detail ?? undefined);
      else this.failTurn(detail, mapped.detail ?? undefined);
      return false;
    }
  }

  /**
   * An `agent_error` that arrives while a send is in flight may be the
   * daemon's own report of that send's failure rather than the agent's
   * voice. Hold it only until the send settles; resolution is by exact
   * text — a frame that does not equal the rejection's raw text is the
   * agent's own prose (a permission failure, a malformed line, the
   * bridge's reattach error) and must always appear.
   */
  private holdAgentError(message: string): void {
    this.heldAgentErrors.push(message);
  }

  /**
   * Remembers one send rejection's raw text until its twin frame consumes it.
   * An entry expires in exactly two ways: its matching frame consumes it, or a
   * push past `MAX_PENDING_SEND_REJECTIONS` evicts the oldest — a refusal the
   * bridge raises before the daemon publishes anything has no twin coming, so
   * without the cap those entries would pile up for the session's life. An
   * evicted twin then shows verbatim: at worst one failure is said twice,
   * never a frame silently dropped — text that matches no entry is never
   * dropped.
   */
  private noteSendRejection(text: string): void {
    this.pendingSendRejections.push(text);
    while (this.pendingSendRejections.length > MAX_PENDING_SEND_REJECTIONS) {
      this.pendingSendRejections.shift();
    }
  }

  /**
   * Consumes the one pending entry `text` is the exact twin of, answering
   * whether it was one. One entry per frame: two same-text failures hold two
   * entries and each of their frames consumes its own.
   */
  private consumeSendRejection(text: string): boolean {
    const index = this.pendingSendRejections.indexOf(text);
    if (index === -1) return false;
    this.pendingSendRejections.splice(index, 1);
    return true;
  }

  /** Resolves every held frame against the pending rejection texts, exact
   * matches only: a twin is consumed (its send already recorded the mapped
   * entry), and anything else — the agent's own prose — is recorded verbatim. */
  private resolveHeldAgentErrors(): void {
    const held = this.heldAgentErrors.splice(0);
    for (const text of held) {
      if (!this.consumeSendRejection(text)) this.noteError(text);
    }
  }

  /**
   * Stores one attachment for this session and answers the reference a later
   * send names it by.
   *
   * The reference is opaque here: its digest is of the bytes **as stored** — the
   * daemon strips metadata before it hashes — so it is a value only the daemon
   * can state and the app only ever hands back.
   *
   * A rejection is thrown rather than swallowed, unlike `interrupt`. The caller
   * is a sequence over a document's pages with a sentence to write about the
   * pages that did not make it, and only the caller has the page number. The
   * error is not wrapped either: `errorSentence` words it for that sentence,
   * and a wrapper would replace the daemon's own reason with a paraphrase of it.
   */
  async depositAttachment(attachment: PromptAttachment): Promise<AttachmentReference> {
    if (this.disposed || !this.started || !this.attached) {
      throw new Error("The agent session is not attached.");
    }
    return this.deps.invoke<AttachmentReference>("session_deposit", {
      id: this.deps.sessionId,
      attachment,
    });
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
   * and reports a refused call through `noteError`: the sentence lands in
   * the transcript and nothing else changes, because a refused switch can
   * land mid-turn and must not collapse the turn.
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
      const mapped = errorSentence(error);
      this.noteError(`Could not switch the model. ${mapped.sentence}`, mapped.detail ?? undefined);
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
   * later session_manifest reports it, and a rejected invoke is reported
   * through `noteError` — the sentence lands in the transcript, the pending
   * marker reverts to the manifest value, and nothing else changes.
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
      const mapped = errorSentence(error);
      this.noteError(`Could not switch the mode. ${mapped.sentence}`, mapped.detail ?? undefined);
      return;
    }
    if (this.disposed || this.state.pendingModeId === null || requestId !== this.modeRequest) {
      return;
    }
    this.armModeTimeout();
  }

  private handleAgentUserMessage(
    event: Extract<SessionEvent, { type: "agent_user_message" }>,
  ): void {
    switch (event.messageKind) {
      case "composer":
        this.appendComposerMessage(event);
        return;
      case "outgoing_a2a":
        this.appendOutgoingA2aMessage(event.text);
        return;
      case "incoming_a2a":
        if (!this.appendPeerMessage(event.text)) this.appendSystemMessage(event.text);
        return;
      case "system_notice":
        if (!this.appendDaemonMessage(event.text)) this.appendSystemMessage(event.text);
        return;
      case "creation":
        this.appendSystemMessage(event.text);
        return;
      case "unknown":
      case undefined:
        // Legacy rows predate `message_kind`; retain their parser-based
        // behavior until those stored rows no longer matter.
        this.handleLegacyAgentUserMessage(event);
        return;
      default:
        // A newer daemon may add a kind this webview does not know yet. Keep
        // the row visible through the same compatibility classifier.
        this.handleLegacyAgentUserMessage(event);
        return;
    }
  }

  private appendComposerMessage(
    event: Extract<SessionEvent, { type: "agent_user_message" }>,
  ): void {
    this.ensureTurn();
    this.closeActiveBlocks();
    this.appendText("user", event.messageId, event.text);
  }

  private appendOutgoingA2aMessage(text: string): void {
    this.closeActiveBlocks();
    this.update({
      items: [
        ...this.state.items,
        {
          id: `a2a-outgoing-message-${this.nextItemId++}`,
          role: "a2a_outgoing_message",
          text,
        },
      ],
    });
  }

  private appendPeerMessage(text: string): boolean {
    const peerMessage = parseAgentPeerMessage(text);
    if (peerMessage === null) return false;
    this.closeActiveBlocks();
    this.update({
      items: [
        ...this.state.items,
        {
          id: `a2a-message-${this.nextItemId++}`,
          role: "a2a_message",
          fromAgent: peerMessage.fromAgent,
          origin: peerMessage.origin,
          body: peerMessage.body,
        },
      ],
    });
    return true;
  }

  private appendDaemonMessage(text: string): boolean {
    const permissionRequest = parseAgentPermissionRequest(text);
    if (permissionRequest !== null) {
      this.closeActiveBlocks();
      this.update({
        items: [
          ...this.state.items,
          {
            id: `permission-request-${this.nextItemId++}`,
            role: "permission_request",
            ...permissionRequest,
          },
        ],
      });
      return true;
    }

    const daemonNotice = parseAgentDaemonNotice(text);
    if (daemonNotice === null) return false;
    this.closeActiveBlocks();
    this.update({
      items: [
        ...this.state.items,
        {
          id: `daemon-notice-${this.nextItemId++}`,
          role: "daemon_notice",
          notice: daemonNotice,
        },
      ],
    });
    return true;
  }

  private handleLegacyAgentUserMessage(
    event: Extract<SessionEvent, { type: "agent_user_message" }>,
  ): void {
    // THE ORDER OF THESE PARSES IS LOAD-BEARING: a permission frame is also
    // a daemon notice, so the structured permission card must win. This is
    // the compatibility path for rows written before `message_kind` existed.
    if (this.appendDaemonMessage(event.text)) return;
    if (this.appendPeerMessage(event.text)) return;
    if ((event.author ?? "human") !== "human") {
      this.appendSystemMessage(event.text);
      return;
    }
    this.appendComposerMessage(event);
  }

  private appendSystemMessage(text: string): void {
    this.closeActiveBlocks();
    this.update({
      items: [
        ...this.state.items,
        {
          id: `system-${this.nextItemId++}`,
          role: "system",
          text,
          severity: "info",
        },
      ],
    });
  }

  handleEvent(event: SessionEvent): void {
    if (this.disposed) return;

    switch (event.type) {
      case "agent_user_message": {
        this.handleAgentUserMessage(event);
        return;
      }
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
        this.setStatus("idle", {
          streaming: false,
          lastFinished: {
            stopReason: event.stopReason,
            ...(event.modelId === undefined ? {} : { modelId: event.modelId }),
            ...(event.usage === undefined ? {} : { usage: event.usage }),
          },
        });
        return;
      case "context_usage":
        // The provider's own reading of this session's context window. It
        // travels the usage lane, not the transcript lane, and a reading
        // identical to the stored one is dropped before anyone is notified —
        // the Codex capture repeats its last frame verbatim.
        this.writeContextUsage(event);
        return;
      case "plan_usage":
        // Account-scoped, so it is recorded beside this session's stream but
        // stored per provider for every session of that provider to read.
        recordPlanUsage(event);
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
        // A notification, not a turn ending: the daemon publishes it for one
        // malformed output line and returns to its read loop, and the turn
        // outcome is recorded from agent_finished/exit instead. So the
        // sentence lands in the transcript and the turn stays open — ending
        // it here rejected paid-for runs. During an in-flight send the frame
        // may instead be the daemon's own report of that send's failure (it
        // publishes, then rejects); hold it for the send's verdict so one
        // failure is one entry.
        const message = event.message || "The agent reported an unknown error.";
        if (this.sendDepth > 0) {
          this.holdAgentError(message);
          return;
        }
        // A rejection that already settled published this very frame on its
        // way out; the mapped entry it recorded is its one record, and this
        // consumes that rejection's pending text. No match means the frame is
        // the agent's own prose and it is always shown.
        if (this.consumeSendRejection(message)) return;
        this.noteError(message);
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
        this.deps.onPermissionResolved?.(event);
        return;
      case "permission_answered":
        // The durable record of every card resolution — a person's, a
        // delegated one, an auto-answer and a cancel alike. The app renders a
        // child's answered count from the roster push that carries it
        // (`DelegationState.answered`), not from this stream, so the agent
        // transcript has nothing to show for it. Listed rather than defaulted
        // so a new daemon event still lands in the `never` guard below.
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
        // A model switch retires the stored reading: for Codex — the only
        // provider whose reading names no model — the number would keep the
        // old model's window after the switch, and nothing else can dislodge
        // it before the next turn's first frame.
        const modelSwitched =
          previous !== null &&
          previous.currentModelId !== undefined &&
          event.currentModelId !== undefined &&
          previous.currentModelId !== event.currentModelId;
        if (modelSwitched) this.writeContextUsage(null);
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
          this.failSession("The agent stopped before finishing this turn.");
        } else {
          this.setStatus("closed", { streaming: false });
        }
        return;
      case "recovered":
        this.stopRunningSubagents();
        this.failSession(RECOVERED_SESSION_UNAVAILABLE);
        return;
      // Our view was replaced, not the session: another client resumed it and
      // the daemon detached this attachment. The session is alive and a fresh
      // attach would work, but this view can no longer speak for it, so input
      // stops here rather than failing on every later send.
      case "detached":
        this.stopRunningSubagents();
        this.failSession("Another client took over this session.");
        return;
      case "output":
      case "agent_stderr":
      case "silent":
      case "sessions_snapshot":
      case "snapshot":
      case "agent_reported":
      // Journaled for audit and not emitted to observers; the transcript gains
      // steer rendering in slice 4b.
      case "steered":
        return;
      case "journal_degraded":
        this.recordJournalLoss(event.droppedFrames, event.droppedBytes);
        return;
      case "agent_created":
        // A created child, recorded on its creator's transcript. Listed so it is
        // not silently dropped, and ignored because no view renders it: the fact
        // the app uses is the child's own row — its `displayName` (the tab's
        // label) and its `createdBy` (the badge naming this session). The one
        // event this pipeline acts on is the finish below.
        return;
      case "child_finished":
        // One created child's finish, on the creator's transcript. The app's
        // only use of it is a Design history entry pointing at the child: the
        // journal is the artifact store and the history holds pointers, so
        // nothing from `event.artifacts` is copied, resolved or fetched here.
        //
        // It is handled in the shared event pipeline on purpose. `child_finished`
        // is published on the CREATOR's session, and the creator's attachment
        // outlives navigation: a Design host with work is deliberately retained,
        // and a retained host keeps its session until the process ends or the
        // user ends it (`designHasWork` in `src/app/App.tsx`, `hostDisposers` in
        // `agentHost.ts`). The live event is therefore the normal arrival, and
        // replay is the recovery path for a finish that had no listener: an app
        // restart, or a host released on navigation because it held no work.
        // Seeing the same finish twice is harmless — the second one writes the
        // same entry over the same time and changes nothing.
        //
        // Nothing on this path may call the daemon: `client.rs` runs its event
        // handlers on the connection's only reader thread, so a synchronous
        // roundtrip from inside an event callback deadlocks until the RPC times
        // out (`attached-connection-loses-events.md`; the dispatcher-thread fix
        // is not in). The history write is the app's own surface settings file
        // through Tauri commands that never touch the daemon — keep it that way.
        // The mirror below is scheduled, not called: its synchronous prefix
        // is the `completed` gate and the sequence bump — no pin reads, no
        // store reads, no daemon I/O — and the replay plus the pin, duplicate
        // and content checks run after this handler returns, so nothing here
        // blocks the event callback and nothing acts — no resume, no spawn,
        // no send. An `onChildFinished` override (the history reopen)
        // suppresses both the write and the mirror: a replayed finish must
        // neither re-date the history nor yank the panel.
        if (this.deps.onChildFinished !== undefined) {
          void this.deps.onChildFinished(event);
        } else {
          void recordChildFinishedHistory(event);
          scheduleDelegatedDesignMirror(event);
        }
        return;
      default: {
        // Every `SessionEvent` arm is a case above, so this branch is
        // unreachable for the protocol as typed: the `never` assignment is a
        // compile-time exhaustiveness check, and it is why `agent_created` and
        // `child_finished` had to be listed instead of falling through. Both
        // were added to the daemon while this switch named neither, and both
        // were dropped in silence for exactly that reason.
        //
        // It is NOT a defence against a newer daemon. `SessionEvent` carries no
        // `#[serde(other)]` (`devboule-protocol/src/session.rs`), so an event
        // this build has no arm for fails to deserialize in the daemon client,
        // which treats any decode failure as fatal and emits a synthetic `Exit`
        // to every subscription (`devboule-daemon/src/client.rs`,
        // `fail_connection`). A newer daemon therefore shows dead sessions in
        // this app, not the sentence below; that sentence is reachable only if
        // something hands this method an object that is not a daemon event.
        const unknownEvent: never = event;
        // The stream delivered something this build cannot interpret; the
        // conservative reading is that the session's view is not trustworthy.
        this.failSession(
          `The daemon sent an unknown session event type: ${eventTypeName(unknownEvent)}.`,
        );
        return;
      }
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
    this.usageListeners.clear();
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

  /**
   * A refused call on a live session: the sentence is recorded and nothing
   * else changes — not the status, not the turn, not the stream. This is for
   * refused model and mode switches, which can land mid-turn; collapsing the
   * turn here would hide the Stop button while the agent keeps working.
   */
  private noteError(message: string, detail?: string): void {
    this.update({
      items: [
        ...this.state.items,
        { id: `error-${this.nextItemId++}`, role: "error", text: message, detail },
      ],
    });
  }

  /**
   * The turn ended badly but the session lives: a send was refused. The
   * status returns to `idle` — unless it is already terminal, which the
   * latch in `setStatus` holds — and the turn is closed.
   */
  private failTurn(message: string, detail?: string): void {
    this.turnOpen = false;
    this.closeActiveBlocks();
    this.setStatus("idle", {
      streaming: false,
      items: [
        ...this.state.items,
        { id: `error-${this.nextItemId++}`, role: "error", text: message, detail },
      ],
    });
  }

  /**
   * Our view of the session is gone: the attach failed, the agent process
   * exited, or the session was recovered by another client. The status
   * latches at `error` and input stays disabled.
   */
  private failSession(message: string, detail?: string): void {
    this.turnOpen = false;
    this.closeActiveBlocks();
    this.setStatus("error", {
      streaming: false,
      items: [
        ...this.state.items,
        { id: `error-${this.nextItemId++}`, role: "error", text: message, detail },
      ],
    });
  }

  /** Worst-known journal loss: a smaller later report cannot un-drop frames. */
  private recordJournalLoss(frames: number, bytes: number): void {
    const previous = this.state.journalLoss;
    this.update({
      journalLoss: {
        frames: Math.max(previous?.frames ?? 0, frames),
        bytes: Math.max(previous?.bytes ?? 0, bytes),
      },
    });
  }

  /**
   * The only writer of `status`. Terminal states latch, but not laterally —
   * the consumer renders them differently (`error` reads "Needs attention",
   * `closed` reads "Finished"), so: `error` is final, because a failure is
   * never relabelled as a clean finish; `closed` may be superseded by
   * `error`, because a takeover revealed after a clean exit is real new
   * information. `update()` forbids `status` at compile time — including a
   * whole-state spread, whose `status` property is `never` here — and the
   * runtime strips one that arrives anyway. `rest` merges in the same
   * notification, so compound transitions stay atomic for listeners that
   * read several fields.
   */
  private setStatus(next: AgentStatus, rest?: AgentSessionPatch): void {
    const from = this.state.status;
    const latched = from === "error" || (from === "closed" && next !== "error");
    const { status: _discarded, ...safeRest } = rest ?? {};
    this.state = {
      ...this.state,
      ...safeRest,
      ...(latched ? {} : { status: next }),
    };
    this.notify();
  }

  private update(patch: AgentSessionPatch): void {
    const { status: _notWritableHere, ...rest } = patch;
    this.state = { ...this.state, ...rest };
    this.notify();
  }

  private notify(): void {
    // A listener may dispose or unsubscribe during notification; a snapshot prevents that
    // mutation from skipping listeners that were already subscribed for this update.
    for (const listener of [...this.listeners]) listener();
  }

  /**
   * The one writer of `state.contextUsage`: it notifies the usage lane and
   * deliberately not the transcript lane — a reading changes the ring, never
   * the transcript — and drops a reading identical to the stored one so a
   * repeated frame notifies no one at all. The field itself lives in `state`
   * so `getState()` stays the single read.
   */
  private writeContextUsage(next: ContextUsage | null): void {
    const current = this.state.contextUsage;
    const identical =
      next !== null &&
      current !== null &&
      current.modelId === next.modelId &&
      current.usedTokens === next.usedTokens &&
      current.maxTokens === next.maxTokens &&
      current.live === next.live;
    if (identical || (next === null && current === null)) return;
    this.state = { ...this.state, contextUsage: next };
    for (const listener of [...this.usageListeners]) listener();
  }
}
