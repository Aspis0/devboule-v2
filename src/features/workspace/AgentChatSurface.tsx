import {
  memo,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  useCallback,
  type ReactNode,
} from "react";
import {
  createSessionChannel,
  sessionAttach,
  sessionClose,
  sessionDeposit,
  sessionDetach,
  sessionInterrupt,
  sessionSend,
  sessionSetFeature,
  sessionSetMode,
  sessionSetModel,
  type AttachmentReference,
  type SubscriptionId,
  type SessionChannel,
} from "../../lib/tauri";
import type {
  ActiveTurnBehavior,
  AgentActivityState,
  Attention,
  DaemonConnectionState,
  PermissionRequest,
  PermissionResolved,
  PromptAttachment,
  Session,
  SessionManifest,
  SessionModel,
  SessionState,
} from "../../types/ipc";
import { AgentSession, lastAssistantMessage, normalizeGoal } from "../../lib/agentSession";
import type { AgentSessionState, AgentStatus } from "../../lib/agentSession";
import { errorSentence } from "../../lib/errorSentence";
import { useConversationScrollStick } from "./useConversationScrollStick";
import { PaneHeader } from "./paneHeader/PaneHeader";
import { headerDisplay } from "./paneHeader/paneHeaderStatus";
import { headerMenu, type HeaderMenuSeam } from "./paneHeader/paneHeaderMenu";
import { getPreferredEffort, setPreferredEffort } from "../../lib/modelPrefs";
import { WorkspaceComposer } from "./WorkspaceComposer";
import { sendChatImagesByReference } from "./chatImageTransport";
import { SubagentMenu, type SubagentArchiveTarget } from "./SubagentMenu";
import { childRow, deriveSubagentRows, isArchivable } from "./subagentRows";
import { SessionContextMeter } from "./ContextMeter";
import { journalLossCopy } from "./journalLoss";
import { PickerChip, modeDotClass } from "../../components/PickerChip";
import type { ChatFileLinks } from "../../lib/chatFilePaths";
import { TurnRail } from "./timeline/TurnRail";
import { TurnFooter } from "./timeline/TurnFooter";
import "./timeline/timeline.css";
import type { A2aNameSource } from "./A2aMessageCard";
import { AgentTaskPill } from "./AgentTaskPill";
import { GoalLine } from "./paneHeader/GoalLine";
import type { WorkspaceCommand } from "./WorkspaceCommandMenu";
import { setHeldAssistantText } from "./attentionNotice";
import { QueueTrack } from "./QueueTrack";
import { useMessageQueue } from "./useMessageQueue";
import {
  getSendBehavior,
  resolveActiveSendBehavior,
  subscribeSendBehavior,
} from "../../lib/sendBehavior";

import { TranscriptRows } from "./transcript/TranscriptRows";

// One classifier owns both whether input is disabled and the sentence explaining it.
export function composerDisabledReason(
  osGone: boolean,
  daemonGone: boolean,
  status: AgentStatus,
): string | null {
  switch (status) {
    case "error":
    case "closed":
      return "This session is no longer available.";
    case "initializing":
      if (osGone) return "This session is no longer available.";
      if (daemonGone) return "The agent daemon is not connected.";
      return "Connecting to the agent…";
    case "idle":
    case "running":
      if (osGone) return "This session is no longer available.";
      if (daemonGone) return "The agent daemon is not connected.";
      return null;
  }
}

/** The universal `/goal` entry: `/goal x` travels to the daemon as plain
 * text, which intercepts it, so the app only makes it discoverable. */
const UNIVERSAL_GOAL_COMMAND: WorkspaceCommand = {
  name: "goal",
  description: "Set, show, or clear this session's goal",
};

/**
 * The slash menu for a live agent session: the provider catalog plus the
 * universal `/goal`, deduped by name without touching the provider's own
 * entry or description (Codex ships one). Terminals never reach this —
 * they mount TerminalSurface, which has no slash menu — and a caller hands
 * false for an ended or recovered session.
 */
export function withGoalCommand(
  commands: readonly WorkspaceCommand[],
  includeGoal: boolean,
): WorkspaceCommand[] {
  if (!includeGoal) return [...commands];
  if (commands.some((command) => command.name.toLowerCase() === "goal")) return [...commands];
  return [...commands, UNIVERSAL_GOAL_COMMAND];
}

/**
 * The command list the composer receives: `/goal` for a live session alone.
 * A starting session counts as live; an ended or recovered one never does.
 */
export function goalCommandsFor(
  commands: readonly WorkspaceCommand[],
  observedState: SessionState | null,
): WorkspaceCommand[] {
  const gone = observedState?.type === "ended" || observedState?.type === "recovered";
  return withGoalCommand(commands, !gone);
}

interface AgentChatSurfaceProps {
  sessionId: string;
  title: string;
  cwd?: string;
  id?: string;
  auxiliary?: ReactNode;
  /** Extra controls at the header's trailing edge, after the status word:
   * the recovered reopen bar on a recovered transcript, nothing on a live
   * pane. The header owns the row; this only fills its end. */
  headerTrailing?: ReactNode;
  onOpenSubagent?: (sessionId: string) => void;
  subagentAttention?: ReadonlyMap<string, string>;
  onRefreshSubagents?: () => Promise<void>;
  /** The kebab's close-group wiring, from the tab-close flow. Absent until the workspace passes it. */
  headerMenuSeam?: HeaderMenuSeam;
  /**
   * Workspace context that renders agent-written file paths as links into
   * the tab opener, exactly the Files panel's open action. Null (or absent)
   * turns recognition off — no workspace, or a surface that cannot open one.
   */
  fileLinks?: ChatFileLinks | null;
  /** The roster's turn status and pending ask, painted by the header. Absent until the workspace passes them. */
  activity?: AgentActivityState;
  attention?: Attention;
  observedState?: SessionState | null;
  /**
   * The roster snapshot's goal: the row's seed before the first frame, and
   * what a stopped or recovered session shows, since those receive no frames.
   */
  initialGoal?: string | null;
  elapsedMs?: number | null;
  /** The daemon connection's state; input is disabled while it cannot carry sends. Required so an omission is compile-visible. */
  daemonState: DaemonConnectionState;
  /**
   * The roster rows the agent-to-agent card resolves a relay's sender against,
   * the subagent pill lists this session's created children from, and the
   * archive act rechecks before each close. Handed none, the card can only
   * show the session id the frame named — the truth it has, minus the name —
   * and the pill lists provider tasks alone.
   */
  sessionRoster?: ReadonlyArray<
    Pick<Session, "displayName" | "id" | "kind" | "title" | "createdBy" | "activity"> & {
      state?: SessionState;
    }
  >;
  /**
   * Device id to display name, the same `DevicesList` map the permission
   * card's origin line resolves against; the a2a card resolves a relay's
   * paired device with it.
   */
  deviceNames?: ReadonlyMap<string, string>;
  /** An unanswered permission card parks this session's turn; Enter's queue
   * action becomes steer while one is open (queueing would strand the message). */
  hasPendingPermission?: boolean;
  /** The one plan timeline row that may stand down: the id of the permission
   * card actually rendered in this pane, while it waits unanswered. Null,
   * every plan row renders. */
  pendingPlanToolCallId?: string | null;
  /**
   * The connected daemon agreed `session.queue`, so it owns this session's
   * queue of unsent follow-ups: the rows come from its snapshots and the
   * actions are its frames. Without it the surface renders no rows, Enter sends
   * as it always has, and the composer's queue action says why it cannot queue.
   */
  queueSupported?: boolean;
  /** The connected daemon agreed `attachments.gif_webp`: the composer may attach GIF and WebP. */
  gifWebpSupported?: boolean;
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
  if (command === "session_deposit")
    return sessionDeposit(id, args?.attachment as PromptAttachment) as Promise<T>;
  if (command === "session_send") {
    // Left off when absent, not passed as an explicit `undefined`, so a send
    // with no attachment keeps the arity every existing caller expects.
    const attachments = args?.attachments as readonly PromptAttachment[] | undefined;
    const text = typeof args?.text === "string" ? args.text : "";
    const subscriptionId = args?.subscriptionId as SubscriptionId;
    // The controller only ever names the two behaviours that differ from a
    // plain send: the composer's replace-a-running-turn, and a steer into one.
    // Anything else is a bug on this side of the wire and is refused loudly:
    // silently dropping it would turn a misspelling into a plain send the
    // caller never asked for.
    const behavior = args?.activeTurnBehavior;
    if (behavior !== undefined && behavior !== "steer" && behavior !== "interrupt") {
      return Promise.reject(new Error(`Unsupported active turn behavior: ${String(behavior)}`));
    }
    const activeTurnBehavior: ActiveTurnBehavior | undefined = behavior;
    // The queue's retry identity, absent for every send that has none. It is
    // the one argument here that a caller may name alone, so the arity ladder
    // below has to count it.
    const key = args?.idempotencyKey;
    const idempotencyKey = typeof key === "string" ? key : undefined;
    // References ride beside the text, never as empty: a send that names no
    // stored attachment is every send that predates the deposit path, and its
    // frame must not grow a key. The controller deposits composer images
    // first and sends what the deposit answered with.
    const rawReferences = args?.attachmentReferences as readonly AttachmentReference[] | undefined;
    const attachmentReferences =
      rawReferences === undefined || rawReferences.length === 0 ? undefined : rawReferences;
    if (
      attachments === undefined &&
      activeTurnBehavior === undefined &&
      attachmentReferences === undefined &&
      idempotencyKey === undefined
    ) {
      return sessionSend(id, subscriptionId, text) as Promise<T>;
    }
    return sessionSend(
      id,
      subscriptionId,
      text,
      attachments,
      activeTurnBehavior,
      attachmentReferences,
      idempotencyKey,
    ) as Promise<T>;
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
  if (command === "session_set_feature") {
    return sessionSetFeature(
      id,
      typeof args?.featureId === "string" ? args.featureId : "",
      args?.enabled === true,
    ) as Promise<T>;
  }
  if (command === "session_interrupt")
    return sessionInterrupt(id, args?.subscriptionId as SubscriptionId) as Promise<T>;
  if (command === "session_detach")
    return sessionDetach(args?.subscriptionId as SubscriptionId) as Promise<T>;
  return Promise.reject(new Error(`Unsupported agent command: ${command}`));
}

function observedType(state: SessionState | null | undefined): SessionState["type"] | null {
  return state?.type ?? null;
}

/** Description line for a model option: catalog copy plus the context size. */
function modelOptionDescription(model: SessionModel): string | undefined {
  const parts = [
    model.description ?? null,
    model.contextTokens === undefined ? null : `${model.contextTokens.toLocaleString()} tokens`,
  ].filter((part): part is string => part !== null);
  return parts.length > 0 ? parts.join(" · ") : undefined;
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

/** The sentence for a model switch, naming the model it is heading toward. */
function pendingModelSentence(manifest: SessionManifest | null, modelId: string): string | null {
  if (manifest === null) return null;
  const model = manifest.models.find((entry) => entry.modelId === modelId);
  return `switching to ${model?.name ?? modelId}…`;
}

/** The sentence for an effort switch, or null when the model offers no such effort. */
function pendingEffortSentence(manifest: SessionManifest | null, effortId: string): string | null {
  if (manifest === null) return null;
  const model = manifestModel(manifest);
  const effort = model?.efforts?.find((entry) => entry.id === effortId);
  return effort === undefined ? null : `switching to ${effort.label}…`;
}

/** The sentence the composer's queue action wears when the connected daemon
 * does not keep a queue for this session. One place, because the button's label
 * and the surface's own copy must not drift. */
export const QUEUE_UNSUPPORTED =
  "The running agent does not keep queued messages for this session.";

export const AgentChatSurface = memo(function AgentChatSurface({
  sessionId,
  title,
  cwd,
  id,
  auxiliary,
  headerTrailing,
  onOpenSubagent,
  subagentAttention,
  onRefreshSubagents,
  headerMenuSeam,
  fileLinks = null,
  activity,
  attention,
  observedState = null,
  initialGoal = null,
  elapsedMs = null,
  daemonState,
  sessionRoster,
  deviceNames,
  hasPendingPermission = false,
  pendingPlanToolCallId = null,
  queueSupported = false,
  gifWebpSupported = false,
  onPermissionRequest,
  onPermissionResolved,
}: AgentChatSurfaceProps) {
  // Undefined until a goal_changed frame arrives; then the last frame, even a clear.
  const goalFrameRef = useRef<string | null | undefined>(undefined);
  const sessionRef = useRef<AgentSession | null>(null);
  const appliedEffortPrefRef = useRef(false);
  const [state, setState] = useState<AgentSessionState>({
    items: [],
    status: "initializing",
    streaming: false,
    availableCommands: [],
    subagents: [],
    lastFinished: null,
    contextUsage: null,
    manifest: null,
    pendingSwitch: null,
    pendingModeId: null,
    journalLoss: null,
    agentTasks: [],
    goal: normalizeGoal(initialGoal),
  });
  const { conversationRef, contentRef, onScroll } = useConversationScrollStick(
    state.items,
    auxiliary,
  );
  // The name source the a2a card resolves against, rebuilt only when a roster
  // the workspace handed down changes: resolution happens at render, so a
  // rename or a re-pairing is visible the next time the card paints.
  const a2aNames = useMemo<A2aNameSource>(() => {
    const sessionById = new Map(
      (sessionRoster ?? []).map((session) => [session.id, session] as const),
    );
    return { sessionById, deviceNames: deviceNames ?? new Map<string, string>() };
  }, [sessionRoster, deviceNames]);

  const subagentRows = useMemo(
    () => deriveSubagentRows(sessionId, sessionRoster, state.subagents),
    [sessionId, sessionRoster, state.subagents],
  );

  // The composer's handed-back draft. `focus` is false for a refused edit whose
  // row took the focus rule in the track, and true for a refusal the user must
  // look at before sending again. `images` ride along when a queued add failed;
  // a failed image send never hands back — its picks stay in the composer while
  // sending and clear only on success.
  const [restoreDraft, setRestoreDraft] = useState<{
    text: string;
    images: readonly PromptAttachment[];
    focus: boolean;
    nonce: number;
  } | null>(null);
  const restoreNonceRef = useRef(0);
  const handDraftBack = useCallback(
    (text: string, focus: boolean, images: readonly PromptAttachment[] = []) => {
      restoreNonceRef.current += 1;
      setRestoreDraft({ text, images, focus, nonce: restoreNonceRef.current });
    },
    [],
  );

  // The daemon's queue for this session: its snapshots in, its five frames out.
  // This surface owns no list and sends nothing queued — the daemon drains its
  // own queue, which is what keeps two devices from sending one row twice.
  const composerQueue = useMessageQueue(
    sessionId,
    useMemo(
      () => ({
        supported: queueSupported,
        activity: activity ?? null,
        // Read at call time: a resume or a reconnect replaces the controller
        // under us, and the newest attach is the one a send may use.
        subscriptionId: () => sessionRef.current?.getSubscriptionId() ?? null,
        depositAttachment: (attachment: PromptAttachment) =>
          sessionRef.current?.depositAttachment(attachment) ??
          Promise.reject(new Error("This view has no session open.")),
        onDraftBack: handDraftBack,
      }),
      [activity, handDraftBack, queueSupported],
    ),
  );

  // The composer takes the focus back when the queue hands it over (an emptied
  // track, a refused steer's or queue's text).
  const composerTextareaRef = useRef<HTMLTextAreaElement | null>(null);
  const captureComposerTextarea = useCallback((element: HTMLTextAreaElement | null) => {
    composerTextareaRef.current = element;
  }, []);
  const focusComposer = useCallback(() => {
    composerTextareaRef.current?.focus();
  }, []);

  // The setting's resolved default. Queueing behind a permission prompt would
  // strand the message, so the queue action becomes steer while a card is open.
  const sendBehavior = useSyncExternalStore(subscribeSendBehavior, getSendBehavior);
  const enterQueues =
    queueSupported && resolveActiveSendBehavior(sendBehavior, hasPendingPermission) === "queue";
  // The composer's one predicate: the roster's activity, or a send of this view
  // that the daemon has not answered yet.
  const turnActive = composerQueue.turnActive;
  const noQueue = queueSupported ? null : QUEUE_UNSUPPORTED;

  const sendSession = useCallback(
    async (
      text: string,
      attachments: readonly PromptAttachment[] = [],
      activeTurnBehavior?: ActiveTurnBehavior,
    ): Promise<boolean> => {
      const session = sessionRef.current;
      if (session === null) return false;
      const submissionId = composerQueue.submissionStarted();
      let replyTurnActive: boolean | undefined;
      const reportTurn = (turnActive: boolean) => {
        replyTurnActive = turnActive;
      };
      try {
        // Composer images travel by reference: one deposit each, then the
        // send names what the deposits answered with. The echo carries the
        // names, so replay resolves the stored bytes instead of the bytes
        // the composer held.
        if (attachments.length === 0) {
          return await session.send(text, [], activeTurnBehavior, [], undefined, reportTurn);
        }
        return await sendChatImagesByReference({
          images: attachments,
          deposit: (attachment) => session.depositAttachment(attachment),
          send: (references) =>
            session.send(text, [], activeTurnBehavior, references, undefined, reportTurn),
        });
      } finally {
        composerQueue.submissionSettled(submissionId, replyTurnActive);
      }
    },
    [composerQueue],
  );

  // The roster goal the workspace keeps current. The controller reads it at
  // construction, so this refresh must stay above the controller effect: a
  // same-commit roster move and generation bump must seed the new goal.
  const latestInitialGoalRef = useRef(initialGoal);
  useEffect(() => {
    latestInitialGoalRef.current = initialGoal;
  });

  // An attachment is valid for exactly one `(sessionId, generation)` pair.
  // Resume keeps the id but increments the generation, so this is the signal
  // that the surface's attachment is dead and must be rebuilt. Generation
  // moves only on resume, so this cannot remount under someone mid-turn.
  useEffect(() => {
    const session = new AgentSession({
      sessionId,
      initialGoal:
        goalFrameRef.current !== undefined ? goalFrameRef.current : latestInitialGoalRef.current,
      invoke: invokeAgentCommand,
      createChannel: createSessionChannel,
      onTurnFinished: composerQueue.onTurnFinished,
      onQueueSnapshot: composerQueue.onSnapshot,
      onGoalChanged: (goal) => {
        goalFrameRef.current = goal;
      },
      onPermissionRequest: onPermissionRequest
        ? (request, subscriptionId) => onPermissionRequest(sessionId, subscriptionId, request)
        : undefined,
      onPermissionResolved: onPermissionResolved
        ? (resolution) => onPermissionResolved(sessionId, resolution)
        : undefined,
    });
    sessionRef.current = session;
    appliedEffortPrefRef.current = false;
    // `start()` is async: seed the new controller now so the old controller's
    // latched error cannot render until the first notification.
    setState(session.getState());
    const unsubscribe = session.subscribe(() => setState(session.getState()));
    void session.start();
    return () => {
      unsubscribe();
      if (sessionRef.current === session) sessionRef.current = null;
      session.dispose();
    };
  }, [
    composerQueue.onSnapshot,
    composerQueue.onTurnFinished,
    onPermissionRequest,
    onPermissionResolved,
    sessionId,
    observedState?.generation,
  ]);

  // The freshest roster handed down: the archive act reads this per close, so
  // a push that landed while the ask was open counts at the moment it matters.
  const rosterRef = useRef(sessionRoster);
  useEffect(() => {
    rosterRef.current = sessionRoster;
  }, [sessionRoster]);

  // The archive act: each target closes only while its roster row is the one
  // the ask captured and is still archivable; a row the roster dropped is
  // already closed.
  const archiveFinishedSubagents = useCallback(
    async (targets: readonly SubagentArchiveTarget[]): Promise<ReadonlyMap<string, string>> => {
      const sentences = new Map<string, string>();
      const closed = new Set<string>();
      for (const target of targets) {
        const roster = rosterRef.current;
        if (roster === undefined) continue;
        const row = roster.find((session) => session.id === target.id);
        if (row === undefined) {
          closed.add(target.id);
          continue;
        }
        // The daemon's close carries no generation: this read is the whole guard.
        const current = childRow(row);
        if (current.generation !== target.generation || !isArchivable(current)) {
          sentences.set(target.id, "It changed since you asked, so it was left open.");
          continue;
        }
        try {
          await sessionClose(target.id);
          closed.add(target.id);
        } catch (cause) {
          sentences.set(target.id, errorSentence(cause).sentence);
        }
      }
      try {
        await onRefreshSubagents?.();
      } catch {
        // The roster read draws its own error line in the workspace.
      }
      const remaining = deriveSubagentRows(
        sessionId,
        rosterRef.current,
        sessionRef.current?.getState().subagents ?? [],
      ).filter((row) => !(row.kind === "child" && closed.has(row.id)));
      // The last row takes the pill and the menu with it: focus has to land
      // somewhere that stays, and a disabled composer cannot take it.
      if (closed.size > 0 && remaining.length === 0) {
        focusComposer();
        const composer = composerTextareaRef.current;
        if (composer === null || composer.disabled || document.activeElement !== composer) {
          conversationRef.current?.focus({ preventScroll: true });
        }
      }
      return sentences;
    },
    // oxlint-disable-next-line react-hooks/exhaustive-deps -- conversationRef comes from the scroll hook: its element is read when the act runs, never at render.
    [focusComposer, onRefreshSubagents, sessionId],
  );

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

  // The toast is worded from what the app already holds, and this surface is
  // the only place a transcript exists: publish the last assistant message
  // while it is on screen, and take the entry back on unmount. A session
  // whose chat is not mounted has no entry, so its toast says the reason
  // alone.
  useEffect(() => {
    setHeldAssistantText(sessionId, lastAssistantMessage(state.items));
    return () => setHeldAssistantText(sessionId, null);
  }, [sessionId, state.items]);

  const manifest = state.manifest;
  const stripModel = manifest === null ? null : manifestModel(manifest);
  const efforts = stripModel?.efforts ?? [];
  const modes = manifest?.modes;
  const currentModeId = state.pendingModeId ?? modes?.currentModeId ?? null;
  const pendingSwitch = state.pendingSwitch;
  // The pending copy belongs on the control that is changing. A combined
  // switch — the session-start path calls setModel with both arguments —
  // paints each control only for the part that differs: the model trigger
  // when the model changes, the effort trigger when the effort changes.
  const pendingModelCopy =
    pendingSwitch !== null &&
    pendingSwitch.modelId !== undefined &&
    pendingSwitch.modelId !== manifest?.currentModelId
      ? pendingModelSentence(manifest, pendingSwitch.modelId)
      : null;
  const pendingEffortCopy =
    pendingSwitch !== null && pendingSwitch.effort !== undefined
      ? pendingEffortSentence(manifest, pendingSwitch.effort)
      : null;
  // The single-model label: the static, non-interactive half of the
  // provider·model pair, carrying both names the manifest line carried.
  const staticModelLabel =
    manifest === null || (stripModel === null && manifest.providerId === undefined)
      ? null
      : [manifest.providerId, stripModel?.name]
          .filter((part): part is string => part !== undefined && part !== null && part !== "")
          .join(" · ");
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
  const header = headerDisplay(observedState, elapsedMs, state.status, activity, attention);
  // The workspace's reopen bar describes this recovered attach state once —
  // while it is shown (a recovered row always shows it), the controller's own
  // attach-state ERROR entry and the composer footer would be second and third
  // tellings of the same fact. Both are about the attach state, not the
  // transcript's history, and both are gone after Reopen's fresh attach; the
  // composer stays disabled either way until then.
  const recoveredAttach = observedType(observedState) === "recovered";
  const lastItem = state.items[state.items.length - 1];
  const streamingThoughtId =
    state.streaming && !osGone && lastItem?.role === "thought" ? lastItem.id : null;
  const disabledReason = composerDisabledReason(osGone, daemonGone, state.status);
  const composerDisabled = disabledReason !== null;
  // Memoised so an `agent_tasks` frame re-renders the pill alone: the
  // element's identity moves only when the checklist does.
  const taskPill = useMemo(
    () => <AgentTaskPill items={state.agentTasks ?? []} />,
    [state.agentTasks],
  );
  // `/goal` rides to the daemon as plain text, which intercepts it: the app
  // only makes it discoverable, for a live agent session alone.
  const composerCommands = useMemo(
    () => goalCommandsFor(state.availableCommands, observedState ?? null),
    // oxlint-disable-next-line react-hooks/exhaustive-deps -- the roster object would churn on every push.
    [state.availableCommands, osGone],
  );
  // Memoized so a streamed token re-renders the transcript, never the rows:
  // the element's identity only moves when the queue's snapshot does.
  const queuedTrack = useMemo(
    () =>
      queueSupported ? (
        <QueueTrack
          items={composerQueue.items}
          onSteer={composerQueue.steerRow}
          onEdit={composerQueue.editRow}
          onDelete={composerQueue.deleteRow}
          onMove={composerQueue.moveRow}
          onEmptied={focusComposer}
        />
      ) : null,
    [
      queueSupported,
      composerQueue.items,
      composerQueue.steerRow,
      composerQueue.editRow,
      composerQueue.deleteRow,
      composerQueue.moveRow,
      focusComposer,
    ],
  );
  return (
    <div id={id} className="workspace-agent-shell" role="tabpanel" aria-label="Agent chat">
      <PaneHeader
        kind="agent"
        title={title || "Agent"}
        display={header}
        menu={headerMenu(cwd, headerMenuSeam, sessionId)}
        trailingSlot={headerTrailing}
        subagentSlot={
          subagentRows.length > 0 ? (
            <SubagentMenu
              rows={subagentRows}
              onOpenSession={onOpenSubagent}
              attentionById={subagentAttention}
              onArchiveFinished={archiveFinishedSubagents}
            />
          ) : null
        }
      />
      {/* Remounted per goal text: the disclosure state belongs to the text,
          so a replaced goal arrives collapsed. */}
      <GoalLine key={state.goal ?? "no-goal"} goal={state.goal ?? null} />
      <div
        ref={conversationRef}
        className="workspace-conversation workspace-scroll"
        onScroll={onScroll}
        tabIndex={-1}
        role="region"
        aria-label="Conversation"
      >
        <div ref={contentRef} className="workspace-conversation-content">
          <TurnRail scrollRef={conversationRef} contentRef={contentRef} items={state.items} />
          {state.items.length === 0 && state.status === "idle" && !osGone ? (
            <div className="workspace-chat-empty">Start a conversation with the agent.</div>
          ) : null}
          <TranscriptRows
            items={state.items}
            recoveredAttach={recoveredAttach}
            pendingPlanToolCallId={pendingPlanToolCallId}
            a2aNames={a2aNames}
            fileLinks={fileLinks}
            transcriptEnded={osGone}
            streamingThoughtId={streamingThoughtId}
          />
          {state.streaming && !osGone ? (
            <div className="workspace-chat-typing" role="status">
              Agent is working
              <span className="workspace-stream-caret" aria-hidden="true" />
            </div>
          ) : null}
          {state.lastFinished !== null ? <TurnFooter finished={state.lastFinished} /> : null}
        </div>
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
      {composerQueue.error !== null ? (
        <div className="workspace-queue-error" role="alert" data-testid="queue-error">
          {composerQueue.error}
        </div>
      ) : null}
      {composerQueue.dropNote !== null ? (
        <div className="workspace-queue-error" role="status" data-testid="queue-drop">
          {composerQueue.dropNote}
        </div>
      ) : null}
      <WorkspaceComposer
        streaming={state.streaming && !osGone}
        turnActive={turnActive}
        queueAllowed={!hasPendingPermission}
        queueUnsupportedReason={noQueue}
        gifWebpSupported={gifWebpSupported}
        disabled={composerDisabled}
        disabledReason={recoveredAttach ? null : disabledReason}
        availableCommands={composerCommands}
        taskPill={taskPill}
        queuedTrack={queuedTrack}
        restoreDraft={restoreDraft}
        onQueue={queueSupported ? composerQueue.queueMessage : undefined}
        enterQueues={enterQueues}
        captureTextarea={captureComposerTextarea}
        onSend={async (text, attachments) => {
          // Images never join a running turn: the daemon refuses a steer
          // that carries them, so an image send waits in the queue behind
          // the turn or starts its own when nothing runs. The answer tells
          // the composer whether its in-flight images may clear: queued
          // transfers and steers clear at once, a refused send keeps them.
          if (attachments.length > 0) {
            if (queueSupported && (turnActive || hasPendingPermission)) {
              composerQueue.queueMessage(text, attachments);
              return true;
            }
            // Images stay in the composer while sending; only the cleared
            // text rides a hand-back on failure, so the retry is whole.
            try {
              const sent = await sendSession(text, attachments);
              if (!sent) handDraftBack(text, true);
              return sent;
            } catch {
              handDraftBack(text, true);
              return false;
            }
          }
          // Text the user types while a turn runs goes straight to the agent as
          // one send that replaces that turn: the wire's own interrupt
          // behaviour, which waits out the turn it displaced on the daemon's
          // side. It never becomes a queued row — nothing is waiting to be
          // sent later, and a queued row could not move until the turn the
          // user is interrupting had already ended.
          const steer = turnActive || hasPendingPermission;
          return sendSession(text, [], steer ? "interrupt" : undefined).then((sent: boolean) => {
            if (!sent) handDraftBack(text, true);
            return sent;
          });
        }}
        onStop={() => void sessionRef.current?.interrupt()}
        contextMeter={
          <SessionContextMeter
            session={sessionRef.current}
            manifest={manifest}
            running={state.streaming && !osGone}
            lastFinished={state.lastFinished}
          />
        }
        controls={
          <>
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
                chipTestId="provider-model-chip"
                optionTestId={(id) => `provider-model-option-${id}`}
                disabled={composerDisabled}
                prefix={manifest.providerId}
                pendingCopy={pendingModelCopy ?? undefined}
              />
            ) : staticModelLabel !== null ? (
              <span className="workspace-picker-static" title={staticModelLabel}>
                {staticModelLabel}
              </span>
            ) : null}
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
            {state.features?.planMode !== undefined ? (
              <PickerChip
                label="Plan mode"
                options={[
                  { id: "on", name: "Plan: On" },
                  { id: "off", name: "Plan: Off" },
                ]}
                currentId={state.features?.planMode ? "on" : "off"}
                onSelect={(id) => void sessionRef.current?.setFeature("planMode", id === "on")}
                chipTestId="plan-mode-chip"
                optionTestId={(id) => `plan-mode-option-${id}`}
                disabled={composerDisabled}
                tooltip="Toggle plan mode"
              />
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
                pendingCopy={pendingEffortCopy ?? undefined}
              />
            ) : null}
          </>
        }
      />
    </div>
  );
});
