import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { NewProjectDialog } from "../../components/NewProjectDialog";
import { SIDE_PANEL_REGISTRY, type SidePanelEntry } from "./sidePanelRegistry";
import { TerminalSurface } from "../terminal/TerminalSurface";
import { AgentChatSurface } from "./AgentChatSurface";
import { HistoryPanel } from "../history/HistoryPanel";
import { CloseConfirm } from "./CloseConfirm";
import { WorkspaceNewTabMenu } from "./WorkspaceNewTabMenu";
import { SessionTabMenu } from "./SessionTabMenu";
import { useTabSelection } from "./useTabSelection";
import { sessionTabElementId, useTabCloseFlow } from "./useTabCloseFlow";
import { discardPersistedPendingCloses, sharedCloseActions } from "./closeActions";
import type { CloseIntent } from "./closePolicy";
import { useWorkspaceDaemon } from "./workspaceDaemon";
import { startPresenceReporting, type PresenceReporter } from "./presence";
import { createDaemonRecovery } from "./daemonRecovery";
import { MAX_PANEL_WIDTH, MIN_PANEL_WIDTH, useWorkspacePanelResize } from "./workspaceResize";
import { useWorkspaceProjects } from "./workspaceProjects";
import { useProviderConsent } from "./useProviderConsent";
import { focusIsWhereTheFlowLeftIt, useStripFocus } from "./stripFocus";
import { AnchoredPopover } from "./popoverPlace";
import { useSelectedTabVisible } from "./stripScroll";
import {
  DELEGATION_CAPABILITY,
  delegationController,
  type DelegationController,
} from "../../lib/delegation";
import {
  PermissionCard as WorkspacePermissionCard,
  formatPermissionCommand,
  resolutionOutcome,
} from "../../components/PermissionCard";
import {
  chatCapableProviders,
  peerDeviceNames,
  requiresConsent,
  sessionAttentionLabel,
  sessionCreateFromProvider,
  sessionCreatorBadge,
  sessionDelegationBadges,
  sessionDelegationTakeBack,
  sessionDisplayNames,
  sessionDotTone,
  sessionOriginBadge,
  sessionOriginUnknown,
  sessionStateLabel,
  sessionTitle,
  isRecoveredSession,
  useWorkspaceSessions,
} from "./workspaceSessions";
import { RecoveredSessionBar } from "./recoveredSessionBar";
import { DaemonRestartNotice } from "./daemonRestartNotice";
import type {
  DaemonStatus,
  PermissionRequest,
  PermissionResolved,
  ProviderInfo,
  Session,
} from "../../types/ipc";
import type { DelegationBadge } from "./workspaceSessions";
import { isAgentKind } from "../../types/ipc";
import {
  daemonRestart,
  devicesList,
  providersList,
  reasonFromCause,
  sessionClose,
  sessionStop,
} from "../../lib/tauri";
import "./Workspace.css";

type ActiveSidePanel = SidePanelEntry["id"];
/**
 * Where the provider choice UI is anchored: a project's "New workspace" row
 * or the tab strip's session "+" button. It only decides placement; the
 * menu itself never reads which button opened it.
 */
type ProviderAnchor = { kind: "project"; projectId: string } | { kind: "strip" };
const WORKSPACE_TERMINAL_PANEL_ID = "workspace-panel-terminal";

function daemonDotTone(state: DaemonStatus["state"]): string {
  if (state === "connected") return "green";
  if (state === "connecting") return "border";
  return "terracotta";
}

function daemonLabel(status: DaemonStatus): string {
  if (status.state === "connected") {
    const pid = status.pid !== null ? `pid ${status.pid}` : "connected";
    return status.message ? `daemon · ${pid} · ${status.message}` : `daemon · ${pid}`;
  }
  if (status.state === "connecting") return "daemon · connecting";
  if (status.state === "unresponsive") {
    // The supervisor's sentence, verbatim — this strip is also what keeps the
    // state visible after the user declines the restart dialog.
    return status.message ? `daemon · ${status.message}` : "daemon · not answering";
  }
  if (status.message) return `daemon · ${status.message}`;
  return "daemon · disconnected";
}

export { WorkspacePermissionCard, formatPermissionCommand };

/** A subscription that never fires: the badge of a panel with no live reads. */
const subscribeNoMeta = () => () => {};

/**
 * One badge list per roster row, cached by row identity: the strip maps over
 * it on every render of the workspace (including every keystroke and daemon
 * status tick), and the rows themselves only change when a push replaces
 * them. Without the cache each render allocated fresh arrays and objects per
 * row for the same answer. The cache can hold a stale list only if a row
 * object were ever mutated in place — Session rows are replaced, never
 * edited — and the entries are tiny.
 */
const delegationBadgeCache = new WeakMap<Session, DelegationBadge[]>();
function cachedDelegationBadges(session: Session): DelegationBadge[] {
  const cached = delegationBadgeCache.get(session);
  if (cached !== undefined) return cached;
  const badges = sessionDelegationBadges(session);
  delegationBadgeCache.set(session, badges);
  return badges;
}

interface WorkspaceProps {
  sidePanelRegistry?: readonly SidePanelEntry[];
  /**
   * The delegation switch's controller, injectable for tests like the panel
   * registry. Defaults to the app's one shared instance: the take-back below
   * and the Settings → Agents switch are two entry points to the same
   * setting, and a value one flipped is what the other must read.
   */
  delegation?: DelegationController;
}

/**
 * One attribution an outside answer carried. Both fields are honest-or-null:
 * `answeredBy` is null when the daemon did not say who — silence is never
 * read as "a person answered" — and `outcome` is null when the daemon did not
 * say what was chosen (absent `selectedOptionKind`, or a kind this build does
 * not know: `allow_always` was exactly the value the old two-way branch
 * rendered as a denial). A resolution is set for EVERY outside answer; the
 * card stays to show it and carries the one control that can clear it.
 */
interface QueueResolution {
  outcome: "allowed" | "denied" | null;
  answeredBy: string | null;
}

export function Workspace({
  sidePanelRegistry = SIDE_PANEL_REGISTRY,
  delegation: delegationControllerProp,
}: WorkspaceProps = {}) {
  const delegation = delegationControllerProp ?? delegationController;
  const {
    visibleProjects,
    loading: projectsLoading,
    error: projectsError,
    selectedWorkspace,
    setSelectedWorkspace,
    setSessionFacts,
    search,
    handleSearchChange,
    addWorkspace,
    projectDialogOpen,
    openProjectDialog,
    closeProjectDialog,
    handleCreateProject,
    newProjectTriggerRef,
    retryProjects,
  } = useWorkspaceProjects();
  const {
    leftWidth,
    rightWidth,
    leftCollapsed,
    rightCollapsed,
    setLeftCollapsed,
    setRightCollapsed,
    startDrag,
    handleResizeKey,
  } = useWorkspacePanelResize();
  const [activeSidePanel, setActiveSidePanel] = useState<ActiveSidePanel>("changes");
  const [historyOpen, setHistoryOpen] = useState(false);
  const [historySearch, setHistorySearch] = useState("");
  const [surfaceMenuOpen, setSurfaceMenuOpen] = useState(false);
  const [appBuild, setAppBuild] = useState(41);
  const [prLabel, setPrLabel] = useState("Open #412 on GitHub");
  const [permissionQueue, setPermissionQueue] = useState<
    Array<{
      sessionId: string;
      subscriptionId: number;
      request: PermissionRequest;
      /** Set when an agent answered this card elsewhere; the card stays to say so. */
      resolution?: QueueResolution;
    }>
  >([]);
  // Device id to display name, for the tab badge that names a peer session's
  // device. One read per daemon connection: the names come from pairing and do
  // not change while the connection lives.
  const [peerNames, setPeerNames] = useState<ReadonlyMap<string, string>>(() => new Map());
  const daemon = useWorkspaceDaemon();
  const {
    sessions,
    selectedSessionId,
    loading: sessionsLoading,
    creating: sessionCreating,
    error: sessionsError,
    refresh: refreshSessions,
    reconnect: reconnectSessions,
    create: createSession,
    select: selectSession,
    open: openSession,
    dismissError: dismissSessionsError,
  } = useWorkspaceSessions(selectedWorkspace);
  useEffect(() => {
    setSessionFacts(sessions);
  }, [sessions, setSessionFacts]);
  // The strip's close acts: fire at once (the undo window is gone), hide the
  // row until the roster confirms, and own each failure by the act that
  // produced it. App-lifetime, like the acts themselves: a fire still in the
  // air when the user switches to Settings lands its error here, and the
  // list is still shown when the Workspace mounts again.
  const [closeActions] = useState(() =>
    sharedCloseActions({
      archive: (id) => sessionStop(id),
      destroy: (id) => sessionClose(id),
    }),
  );
  const closingIds = useSyncExternalStore(closeActions.subscribe, closeActions.getClosingSnapshot);
  const closeFailures = useSyncExternalStore(
    closeActions.subscribe,
    closeActions.getFailuresSnapshot,
  );
  const visibleSessions = useMemo(() => {
    const hiding = new Set(closingIds);
    return sessions.filter((session) => !hiding.has(session.id));
  }, [sessions, closingIds]);
  // The strip scrolls its selected tab into full view. The arithmetic and the
  // effect live in stripScroll.ts, unit-tested there — happy-dom computes no
  // layout to prove them against here.
  const stripScrollportRef = useRef<HTMLDivElement>(null);
  useSelectedTabVisible(stripScrollportRef, selectedSessionId, visibleSessions);
  // One close, one act: the flow has already asked where the policy says so
  // and resolved its targets; what lands here fires now, a target that went
  // stale between the ask and the click is reported, never touched, and a
  // refused act names itself back to the flow (its row came back).
  const runClose = useCallback(
    (
      kind: CloseIntent,
      matched: readonly Session[],
      skipped: ReadonlyArray<{ id: string; title: string; generation: number }>,
      onFailed?: (sessionId: string) => void,
    ) => {
      for (const session of matched) {
        closeActions.act(
          kind,
          {
            id: session.id,
            title: sessionTitle(session),
            generation: session.state.generation,
          },
          onFailed === undefined ? undefined : () => onFailed(session.id),
        );
      }
      for (const target of skipped) closeActions.skipped(kind, target);
    },
    [closeActions],
  );
  // Multi-select (ours) and the tab close flow (Paseo's menu plus the
  // confirmation) live in their own files; the strip only wires handlers.
  // The "+" button's ref is the flow's last focus fallback (no active tab).
  const addButtonRef = useRef<HTMLButtonElement>(null);
  const {
    selection,
    announcement: selectionAnnouncement,
    handleTabClick,
    clearSelection,
  } = useTabSelection({ sessions: visibleSessions, selectedSessionId, selectSession });
  const tabClose = useTabCloseFlow({
    sessions: visibleSessions,
    selectedSessionId,
    selection,
    onClose: runClose,
    selectSession,
    clearSelection,
    addButtonRef,
  });
  // The roster is the authority on what a close is still hiding: a row it
  // shows as gone, ended, or resumed is confirmed (or moot), and the mark
  // comes off so the strip never hides a session on its own say-so.
  useEffect(() => {
    closeActions.pruneConfirmed(sessions);
  }, [closeActions, sessions]);
  // Records an OLDER build persisted for its undo window must never act:
  // dropped unread, and the key removed, on startup. The storage ACCESS is
  // the helper's problem — inside its try, where a restricted WebView's
  // getter belongs.
  useEffect(() => {
    discardPersistedPendingCloses(() =>
      typeof localStorage !== "undefined" ? localStorage : null,
    );
  }, []);
  // An unknown id means persisted state points to a removed panel, including a plugin that is no
  // longer loaded. Keep that id so the fallback is not shown as the user's selected option; use
  // the first available entry only because rendering safe panel content is better than a blank side panel.
  const selectedSurface =
    sidePanelRegistry.find((surface) => surface.id === activeSidePanel) ??
    sidePanelRegistry[0] ??
    SIDE_PANEL_REGISTRY[0];
  // The selected panel's badge: its own live label when the panel reports one
  // (the Changes panel, while it is open and polling), the registry's static
  // label otherwise. Subscribed only to the selected panel — an unmounted panel
  // reports nothing, which is exactly DECISIONS §3's "last value known".
  const surfaceMeta = useSyncExternalStore(
    selectedSurface.liveMeta?.subscribe ?? subscribeNoMeta,
    () => selectedSurface.liveMeta?.snapshot(selectedWorkspace) ?? selectedSurface.meta,
  );
  const selectedSession = sessions.find((session) => session.id === selectedSessionId) ?? null;
  // The names the roster carries, for the badge that resolves a child's
  // `createdBy` back to the session that created it. Memoized on the roster:
  // the map is a projection of the same array the tab strip maps over, and
  // rebuilding it on unrelated renders bought nothing.
  const sessionNames = useMemo(() => sessionDisplayNames(sessions), [sessions]);
  // The recovery decision is a small external store: it holds the episode, the
  // roster answer, and the attempt-failed note, which only change from pushed
  // updates. Both pushes happen in effects below — no ref is read during render.
  const [daemonRecovery] = useState(() => createDaemonRecovery({ restart: () => daemonRestart() }));
  const restartFailureNote = useSyncExternalStore(daemonRecovery.subscribe, daemonRecovery.note);
  useEffect(() => {
    daemonRecovery.setRoster(sessions.some((session) => session.state.type === "live"));
  }, [sessions, daemonRecovery]);
  useEffect(() => {
    daemonRecovery.onStatus(daemon);
  }, [daemon, daemonRecovery]);
  // Until the daemon first answers "connected" the IPC pipe is not open, so a
  // startup load would race it and latch errors. Projects load exactly once
  // per connected transition; the session controller refreshes on mount and
  // is reloaded on the same transitions — first connect and every reconnect
  // after a daemon restart. A successful load clears its own error.
  const wasConnectedRef = useRef(false);
  const refreshPeerNames = useCallback(async () => {
    try {
      setPeerNames(peerDeviceNames((await devicesList()).peers));
    } catch {
      // Keep the names already known. A badge that falls back to the device id
      // is better than one that disappears because a list read did not answer.
    }
  }, []);
  useEffect(() => {
    if (daemon.state !== "connected") {
      wasConnectedRef.current = false;
      return;
    }
    if (wasConnectedRef.current) return;
    wasConnectedRef.current = true;
    void retryProjects();
    void reconnectSessions();
    void refreshPeerNames();
  }, [daemon.state, reconnectSessions, refreshPeerNames, retryProjects]);
  // Presence reporter lives outside React state: it holds no render output.
  // Selection changes arrive through the second effect below.
  const presenceReporterRef = useRef<PresenceReporter | null>(null);
  useEffect(() => {
    const reporter = startPresenceReporting();
    presenceReporterRef.current = reporter;
    return () => {
      presenceReporterRef.current = null;
      reporter.dispose();
    };
  }, []);
  useEffect(() => {
    presenceReporterRef.current?.onSelectionChanged(selectedSessionId);
  }, [selectedSessionId]);
  const handleReopenSession = useCallback(
    (session: Session) => {
      openSession(session);
      setHistoryOpen(false);
      setHistorySearch("");
    },
    [openSession],
  );
  // A failed resume leaves the row's verdict changed on the daemon side; the
  // bar must not keep its offer on the roster data this surface already held.
  const handleResumeFailed = useCallback(() => {
    void refreshSessions();
  }, [refreshSessions]);
  const handleAppReload = useCallback(() => setAppBuild((build) => build + 1), []);
  const handleOpenPullRequest = useCallback(() => setPrLabel("Opened #412 on GitHub"), []);
  const [providerPicker, setProviderPicker] = useState<ProviderInfo[] | null>(null);
  const [providerAnchor, setProviderAnchor] = useState<ProviderAnchor | null>(null);
  const [newTabMenuOpen, setNewTabMenuOpen] = useState(false);
  const [providerChoosing, setProviderChoosing] = useState(false);
  const providerChoiceInFlightRef = useRef(false);
  const afterProviderChoiceRef = useRef<((provider: ProviderInfo | undefined) => void) | null>(
    null,
  );
  const providerPickerRef = useRef<HTMLDivElement>(null);
  /** The button a provider flow was opened from: the portal positions off its rectangle. */
  const providerAnchorElRef = useRef<HTMLElement | null>(null);
  /** The workspace the open choice was made under: switching it must end the choice. */
  const choiceWorkspaceRef = useRef<string | null>(selectedWorkspace);
  const consentConfirmRef = useRef<HTMLButtonElement>(null);
  const consentRestoreRef = useRef<HTMLButtonElement | null>(null);
  const [providerError, setProviderError] = useState<string | null>(null);
  // A provider choice is a create in waiting: between the click and the
  // chosen provider's create, the strip must not start another session —
  // the shared controller would drop it silently.
  const endProviderChoice = useCallback(() => {
    providerChoiceInFlightRef.current = false;
    setProviderChoosing(false);
  }, []);
  const loadChatProviders = useCallback(async (): Promise<ProviderInfo[]> => {
    const catalog = await providersList();
    return chatCapableProviders(catalog.providers);
  }, []);
  const startAgentSession = useCallback(
    (provider: ProviderInfo | undefined, workspaceId: string | null) => {
      const args = sessionCreateFromProvider(provider);
      void createSession(args.kind, args.provider, workspaceId);
    },
    [createSession],
  );
  const createWorkspaceAndAgent = useCallback(
    async (projectId: string, provider: ProviderInfo | undefined) => {
      const workspace = await addWorkspace(projectId);
      if (workspace !== null) startAgentSession(provider, workspace.id);
      endProviderChoice();
    },
    [addWorkspace, endProviderChoice, startAgentSession],
  );
  const addSessionToWorkspace = useCallback(
    (provider: ProviderInfo | undefined) => {
      startAgentSession(provider, selectedWorkspace);
      endProviderChoice();
    },
    [endProviderChoice, selectedWorkspace, startAgentSession],
  );
  const handleConsentConfirmed = useCallback((provider: ProviderInfo) => {
    setProviderPicker(null);
    setProviderAnchor(null);
    const afterChoice = afterProviderChoiceRef.current;
    afterProviderChoiceRef.current = null;
    afterChoice?.(provider);
  }, []);
  const {
    pending: consentProvider,
    request: requestConsent,
    confirm: consentConfirm,
    cancel: cancelProviderConsent,
    inFlight: consentInFlight,
    commandLine: consentCommandLine,
  } = useProviderConsent({ onConfirmed: handleConsentConfirmed });
  /**
   * One provider-choice flow for every button that starts an agent: 0 capable
   * providers fall straight through, one needs consent when it is an npx
   * wrapper, two or more open the picker. What happens after the choice is
   * the caller's `afterChoice`; the picker and consent card never read it.
   */
  const chooseProvider = useCallback(
    async (
      anchor: ProviderAnchor,
      afterChoice: (provider: ProviderInfo | undefined) => void,
      trigger?: HTMLButtonElement,
    ) => {
      if (providerChoiceInFlightRef.current) return;
      providerChoiceInFlightRef.current = true;
      // The workspace this choice belongs to, captured the moment it starts:
      // switching to another one mid-flow ends the choice (the effect below).
      choiceWorkspaceRef.current = selectedWorkspace;
      setProviderChoosing(true);
      setProviderError(null);
      let capable: ProviderInfo[];
      try {
        capable = await loadChatProviders();
      } catch (cause: unknown) {
        endProviderChoice();
        setProviderError(reasonFromCause(cause));
        return;
      }
      if (capable.length === 0) {
        endProviderChoice();
        afterChoice(undefined);
        return;
      }
      if (capable.length === 1 && !requiresConsent(capable[0])) {
        endProviderChoice();
        afterChoice(capable[0]);
        return;
      }
      afterProviderChoiceRef.current = afterChoice;
      providerAnchorElRef.current = trigger ?? addButtonRef.current;
      setProviderAnchor(anchor);
      if (capable.length === 1) {
        // With a single npx provider no picker opens, so this triggering
        // button is the only focus anchor; the consent effect restores it on
        // cancel (and the picker path sets its own anchor in pickProvider).
        consentRestoreRef.current = trigger ?? null;
        requestConsent(capable[0]);
        return;
      }
      setProviderPicker(capable);
    },
    [endProviderChoice, loadChatProviders, requestConsent, selectedWorkspace],
  );
  const handleNewWorkspace = useCallback(
    (trigger: HTMLButtonElement, projectId: string) => {
      void chooseProvider(
        { kind: "project", projectId },
        (provider) => void createWorkspaceAndAgent(projectId, provider),
        trigger,
      );
    },
    [chooseProvider, createWorkspaceAndAgent],
  );
  const handleNewSession = useCallback(
    (trigger: HTMLButtonElement | null) => {
      void chooseProvider({ kind: "strip" }, addSessionToWorkspace, trigger ?? undefined);
    },
    [addSessionToWorkspace, chooseProvider],
  );
  const dismissNewTabMenu = useCallback(() => setNewTabMenuOpen(false), []);
  // The menu's Agent entry: the flow the "+" owned before the menu, focused
  // from the "+" itself so a cancelled consent card hands focus back to it.
  const handleNewTabAgent = useCallback(() => {
    dismissNewTabMenu();
    const trigger = addButtonRef.current;
    trigger?.focus();
    handleNewSession(trigger);
  }, [dismissNewTabMenu, handleNewSession]);
  // The strip's focus rule and the Terminal entry's focus request live in
  // stripFocus.ts — one rule, one comment, one place to change it.
  const addDisabled = sessionCreating || providerChoosing;
  const { noteChoiceDismissed, terminalAutoFocus, armTerminalFocus, takeTerminalFocus } =
    useStripFocus({
      addButtonRef,
      addDisabled,
      sessionsError,
      providerError,
      pickerOpen: providerPicker !== null,
      selectedSessionId,
      workspaceId: selectedWorkspace,
    });
  // Asked by the terminal surface at the moment it would focus: focus may
  // only move if it is still where this flow left it (body, null, or "+").
  const mayTakeTerminalFocus = useCallback(
    () => focusIsWhereTheFlowLeftIt(document.activeElement, addButtonRef.current),
    [],
  );
  // Terminal closes the menu exactly like Agent and needs a selected
  // workspace (the entry is disabled without one). A refused create hands
  // focus back to "+" through the rule above; a successful one arms the new
  // tab's terminal to take focus when its view opens — armed for the
  // workspace the create ran under, so switching away cancels it.
  const handleNewTabTerminal = useCallback(() => {
    dismissNewTabMenu();
    void createSession("terminal", null, selectedWorkspace).then((session) => {
      if (session !== null) armTerminalFocus(session.id, selectedWorkspace);
    });
  }, [armTerminalFocus, createSession, dismissNewTabMenu, selectedWorkspace]);
  const consentCancel = useCallback(() => {
    // The picker stays anchored behind the consent card; cancelling only
    // removes the card and returns to the option list.
    endProviderChoice();
    cancelProviderConsent();
  }, [cancelProviderConsent, endProviderChoice]);
  useEffect(() => {
    if (consentProvider !== null) {
      consentConfirmRef.current?.focus({ preventScroll: true });
    } else {
      consentRestoreRef.current?.focus({ preventScroll: true });
      consentRestoreRef.current = null;
    }
  }, [consentProvider]);
  const pickProvider = useCallback(
    (provider: ProviderInfo, trigger: HTMLButtonElement) => {
      if (requiresConsent(provider)) {
        consentRestoreRef.current = trigger;
        requestConsent(provider);
        return;
      }
      setProviderPicker(null);
      setProviderAnchor(null);
      const afterChoice = afterProviderChoiceRef.current;
      afterProviderChoiceRef.current = null;
      afterChoice?.(provider);
    },
    [requestConsent],
  );
  const dismissProviderPicker = useCallback(() => {
    afterProviderChoiceRef.current = null;
    // Escape and outside clicks: the choice ends with NO choice, which the
    // strip rule treats exactly like a failure — focus back to "+" if lost.
    noteChoiceDismissed();
    endProviderChoice();
    setProviderPicker(null);
    setProviderAnchor(null);
  }, [endProviderChoice, noteChoiceDismissed]);
  const dismissPickerFlow = useCallback(() => {
    // One close for every way the flow dies without a choice: window resize,
    // an ancestor scroll, a lost anchor, a workspace switch. The card goes
    // too, and dismissing arms the focus rule exactly like Escape does.
    if (consentProvider !== null) consentCancel();
    dismissProviderPicker();
  }, [consentCancel, consentProvider, dismissProviderPicker]);
  // A choice opened under one workspace must never create in another: a
  // pointer click on the row dismisses through the outside rule, but a
  // keyboard- or state-driven switch has no click to catch — the flow ends
  // here, in the workspace it was opened under, never the one now selected.
  useEffect(() => {
    if (!providerChoosing) return;
    if (choiceWorkspaceRef.current !== selectedWorkspace) dismissPickerFlow();
  }, [providerChoosing, selectedWorkspace, dismissPickerFlow]);
  useEffect(() => {
    if (providerAnchor === null && consentProvider === null) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        if (consentProvider !== null) {
          consentCancel();
        } else {
          dismissProviderPicker();
        }
      }
    };
    const onPointer = (event: MouseEvent) => {
      const root = providerPickerRef.current;
      // The anchor is not outside its own popover's world: pressing the
      // trigger that opened the flow must not dismiss it and restart it
      // (the menu's handler has checked its trigger the same way all along).
      if (
        event.target instanceof Node &&
        providerAnchorElRef.current?.contains(event.target) === true
      ) {
        return;
      }
      if (root !== null && event.target instanceof Node && !root.contains(event.target)) {
        if (consentProvider !== null) {
          consentCancel();
        } else {
          dismissProviderPicker();
        }
      }
    };
    // Open over a viewport that then moved is stale: close it (the brief
    // picked closing over repositioning). Dismissing arms the focus rule,
    // so focus the unmount dropped comes back to "+".
    const onResize = () => {
      dismissPickerFlow();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onPointer);
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onPointer);
      window.removeEventListener("resize", onResize);
    };
  }, [consentCancel, consentProvider, dismissProviderPicker, dismissPickerFlow, providerAnchor]);
  const handleSessionClosed = useCallback(() => {
    void refreshSessions();
  }, [refreshSessions]);
  const handlePermissionRequest = useCallback(
    (sessionId: string, subscriptionId: number, request: PermissionRequest) => {
      setPermissionQueue((queue) => {
        const index = queue.findIndex(
          (item) => item.sessionId === sessionId && item.request.toolCallId === request.toolCallId,
        );
        if (index === -1) return [...queue, { sessionId, subscriptionId, request }];
        // A remounted surface re-attaches with a fresh subscription id; the
        // queued card must adopt it or its response reaches the daemon with
        // a dead id.
        if (queue[index].subscriptionId === subscriptionId) return queue;
        const next = [...queue];
        next[index] = { ...next[index], subscriptionId };
        return next;
      });
    },
    [],
  );
  // A card the human answered through this app's own card: it leaves the
  // queue, exactly as it always did.
  const dismissResolvedPermission = useCallback((sessionId: string, toolCallId: string) => {
    setPermissionQueue((queue) =>
      queue.filter(
        (item) => !(item.sessionId === sessionId && item.request.toolCallId === toolCallId),
      ),
    );
  }, []);
  // A card resolved from somewhere else. The card NEVER leaves on the strength
  // of this event alone, and the event's silence is never read as "a person
  // answered": the daemon naming nobody leaves the answer unnamed, on screen,
  // with its outcome claimed only if the option kind named one. A denial by an
  // agent is exactly the event a human reviewing the roster needs to see
  // happened — and a card vanishing as if they had answered it themselves is
  // the one rendering a consent surface may not do with an unnamed answer.
  const handlePermissionResolved = useCallback(
    (sessionId: string, resolution: PermissionResolved) => {
      setPermissionQueue((queue) => {
        const index = queue.findIndex(
          (item) =>
            item.sessionId === sessionId &&
            item.request.toolCallId === resolution.toolCallId &&
            item.resolution === undefined,
        );
        if (index === -1) return queue;
        const answeredBy = resolution.answeredBy?.trim() || null;
        const outcome = resolutionOutcome(resolution.selectedOptionKind);
        const next = [...queue];
        next[index] = { ...next[index], resolution: { outcome, answeredBy } };
        return next;
      });
    },
    [],
  );
  // The take-back and the child rows read the one switch. Fetched here — not
  // only in Settings — so the roster's control is right even if Settings was
  // never opened. Capability-gated like every delegation RPC: a daemon that
  // never advertised `permission_delegation` is never asked.
  //
  // The read is keyed on the daemon's IDENTITY, not just this component's
  // mount (audit 3, F2): a daemon restart — even one the 2 s poll never saw
  // as a gap — changes `instanceId`, and every reconnect transitions
  // `state`. Either way the store's answer is re-asked, because a connection
  // that dropped and returned means every cached answer is a guess, and the
  // `delegation.json` the app's own `source: "file"` sentence advertises
  // moves the daemon's value with no app-side event.
  const delegationState = useSyncExternalStore(delegation.subscribe, delegation.getState);
  const delegationSupported = daemon.capabilities.includes(DELEGATION_CAPABILITY);
  useEffect(() => {
    if (!delegationSupported || daemon.state !== "connected") return;
    void delegation.load();
  }, [delegation, delegationSupported, daemon.state, daemon.instanceId]);
  // The honest gate for the control that stops delegation (audit 3, F2): it
  // is hidden only when the store POSITIVELY holds `false` — a value it can
  // hold only from a daemon answer or an accepted write, never from silence —
  // and it stays visible in the unknown state (`null`), where hiding it would
  // let a stale belief withdraw the one control that corrects it. The write
  // itself is the honest action from unknown: `false` needs no stored answer
  // (see `setEnabled` in `lib/delegation.ts`), so the click acts instead of
  // decorating a maybe.
  const takeBackAvailable = delegationSupported && delegationState.enabled !== false;
  const takeBack = useCallback(() => {
    void delegation.setEnabled(false);
  }, [delegation]);
  // The panel's slot is for the card that needs the human: the first
  // UNRESOLVED card of the session. A resolved card at the head must not
  // hide a waiting one behind it (re-audit F6) — a resolved card offers only
  // Clear, so find-on-head made the waiting card's Allow/Deny unreachable
  // and said nothing about a second card existing. When nothing waits, the
  // resolved card stays on screen: it never vanishes on the strength of the
  // resolution event alone, and Clear is its removal path.
  const selectedPermission =
    permissionQueue.find(
      (item) => item.sessionId === selectedSessionId && item.resolution === undefined,
    ) ??
    permissionQueue.find((item) => item.sessionId === selectedSessionId) ??
    null;
  const sessionStatusText = sessionsError
    ? sessionsError
    : sessionCreating
      ? "Starting session…"
      : sessionsLoading && sessions.length === 0
        ? "Loading sessions…"
        : `${sessions.length} session${sessions.length === 1 ? "" : "s"}`;

  // One instance of the provider choice UI, anchored where the flow was
  // opened. It renders only the choice and consent; what happens afterwards
  // was fixed when the flow started.
  const providerMenu =
    providerAnchor === null || (providerPicker === null && consentProvider === null) ? null : (
      <AnchoredPopover
        containerRef={providerPickerRef}
        anchorRef={providerAnchorElRef}
        onDismiss={dismissPickerFlow}
        className="workspace-surface-menu"
        role={consentProvider !== null ? "group" : "listbox"}
        aria-label={consentProvider !== null ? "Confirm agent" : "Choose agent"}
      >
        {consentProvider !== null ? (
          <>
            <div className="workspace-menu-label">This agent downloads third-party code</div>
            <div className="workspace-surface-options">
              <div className="workspace-consent-provider">
                <span className="workspace-surface-name">{consentProvider.id}</span>
                <span className="workspace-consent-spec" id="workspace-consent-command">
                  {consentCommandLine}
                </span>
              </div>
              <p className="workspace-consent-notice" id="workspace-consent-warning">
                npx will download and run third-party code on first use.
              </p>
            </div>
            <div className="workspace-consent-actions">
              <button type="button" className="workspace-secondary-action" onClick={consentCancel}>
                Cancel
              </button>
              {/*
                Focus moves here when the card opens, so this button's accessible
                description is the whole of what a screen-reader user hears before
                approving. Without it they hear "Confirm" and nothing about the
                command or the download — which is not consent. The command comes
                first because it is the specific thing being approved.
              */}
              <button
                ref={consentConfirmRef}
                type="button"
                className="workspace-primary-action"
                onClick={consentConfirm}
                disabled={consentInFlight}
                aria-describedby="workspace-consent-command workspace-consent-warning"
              >
                Confirm
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="workspace-menu-label">Choose agent</div>
            {[
              {
                label: "Installed",
                providers: providerPicker!.filter((provider) => !requiresConsent(provider)),
              },
              {
                label: "Available to install",
                providers: providerPicker!.filter((provider) => requiresConsent(provider)),
              },
            ]
              .filter((group) => group.providers.length > 0)
              .map((group) => (
                <div className="workspace-provider-group" key={group.label}>
                  <div className="workspace-menu-label">{group.label}</div>
                  <div className="workspace-surface-options">
                    {group.providers.map((provider) => (
                      <button
                        type="button"
                        role="option"
                        className="workspace-surface-option"
                        key={provider.id}
                        onClick={(event) => {
                          pickProvider(provider, event.currentTarget);
                        }}
                      >
                        <span className="workspace-surface-name">{provider.id}</span>
                      </button>
                    ))}
                  </div>
                </div>
              ))}
          </>
        )}
      </AnchoredPopover>
    );

  return (
    <section className="workspace-screen" data-screen-label="Workspace">
      <aside
        className="workspace-panel workspace-left-panel"
        style={{ width: leftCollapsed ? "30px" : `${leftWidth}px` }}
        aria-label={historyOpen ? "History" : "Workspaces"}
      >
        {leftCollapsed ? (
          <button
            type="button"
            className="workspace-collapsed-panel"
            onClick={() => setLeftCollapsed(false)}
            title="Show workspaces"
            aria-label="Show workspaces"
          >
            <span aria-hidden="true">›</span>
            <span className="workspace-vertical-label">workspaces</span>
          </button>
        ) : (
          <div className="workspace-panel-open">
            <div className="workspace-left-toolbar">
              <button
                type="button"
                className="workspace-icon-button"
                onClick={() => setLeftCollapsed(true)}
                title="Collapse"
                aria-label="Collapse workspaces"
              >
                ‹
              </button>
              <label className="workspace-search">
                <span className="sr-only">
                  {historyOpen ? "Search history" : "Search workspaces"}
                </span>
                <input
                  value={historyOpen ? historySearch : search}
                  onChange={(event) => {
                    if (historyOpen) {
                      setHistorySearch(event.target.value);
                    } else {
                      handleSearchChange(event);
                    }
                  }}
                  placeholder="Search"
                />
              </label>
              <button
                type="button"
                className="workspace-add-button"
                ref={newProjectTriggerRef}
                onClick={openProjectDialog}
                title="New project"
                aria-label="New project"
              >
                +
              </button>
            </div>

            <div className="workspace-scroll workspace-project-list">
              {historyOpen ? (
                <HistoryPanel search={historySearch} onReopen={handleReopenSession} />
              ) : (
                <>
                  {projectsLoading ? (
                    <div className="workspace-empty" role="status">
                      Loading projects…
                    </div>
                  ) : null}
                  {projectsError !== null ? (
                    <div className="workspace-project-error" role="alert">
                      {projectsError}
                      <button
                        type="button"
                        className="workspace-secondary-action"
                        onClick={() => void retryProjects()}
                      >
                        Retry
                      </button>
                    </div>
                  ) : null}
                  {providerError !== null ? (
                    <div className="workspace-project-error" role="alert">
                      {providerError}
                    </div>
                  ) : null}
                  {visibleProjects.map((project) => (
                    <div className="workspace-project" key={project.id}>
                      <div className="workspace-project-heading">
                        <span>{project.name}</span>
                        <button
                          type="button"
                          className="workspace-project-add"
                          onClick={(event) =>
                            void handleNewWorkspace(event.currentTarget, project.id)
                          }
                          title="New workspace in this project"
                          aria-label={`New workspace in ${project.name}`}
                        >
                          +
                        </button>
                      </div>
                      {project.workspaceError !== undefined ? (
                        <div className="workspace-project-error" role="alert">
                          Could not load this project&apos;s workspaces: {project.workspaceError}
                          <button
                            type="button"
                            className="workspace-secondary-action"
                            onClick={() => void retryProjects()}
                          >
                            Retry
                          </button>
                        </div>
                      ) : null}
                      <div className="workspace-project-items">
                        {project.workspaces.map((workspace) => (
                          <button
                            type="button"
                            className={`workspace-row${selectedWorkspace === workspace.id ? " workspace-row-selected" : ""}`}
                            key={workspace.id}
                            onClick={() => setSelectedWorkspace(workspace.id)}
                            aria-pressed={selectedWorkspace === workspace.id}
                            title={workspace.path ? workspace.path : undefined}
                          >
                            <span
                              className={`workspace-status-dot workspace-dot-${workspace.dotTone}`}
                            />
                            <span className="workspace-row-copy">
                              <span className="workspace-row-title">{workspace.title}</span>
                              <span className="workspace-row-meta">{workspace.meta}</span>
                            </span>
                            <span className="workspace-isolation">{workspace.isolation}</span>
                          </button>
                        ))}
                        <div className="workspace-new-row-wrap">
                          <button
                            type="button"
                            className="workspace-new-row"
                            onClick={(event) =>
                              void handleNewWorkspace(event.currentTarget, project.id)
                            }
                          >
                            <span aria-hidden="true">+</span>New workspace
                          </button>
                          {providerAnchor?.kind === "project" &&
                          providerAnchor.projectId === project.id
                            ? providerMenu
                            : null}
                        </div>
                      </div>
                    </div>
                  ))}
                  {projectsError === null && !projectsLoading && visibleProjects.length === 0 ? (
                    <div className="workspace-empty">No matching workspaces</div>
                  ) : null}
                </>
              )}
            </div>

            <div className="workspace-sidebar-footer">
              <button
                type="button"
                className="workspace-history-button"
                aria-pressed={historyOpen}
                aria-controls="workspace-history-panel"
                onClick={() => setHistoryOpen((open) => !open)}
                title={historyOpen ? "Show workspaces" : "Show history"}
              >
                History
              </button>
              <div className="workspace-daemon-status" title={daemon.message ?? undefined}>
                <span
                  className={`workspace-status-dot workspace-dot-${daemonDotTone(daemon.state)}`}
                />
                <span className="workspace-daemon-status-label">{daemonLabel(daemon)}</span>
                {restartFailureNote !== null ? (
                  <span className="workspace-recovery-note">{restartFailureNote}</span>
                ) : null}
              </div>
            </div>
          </div>
        )}
      </aside>

      <button
        type="button"
        className="workspace-resize-handle"
        onMouseDown={(event) => startDrag("left", event)}
        onDoubleClick={() => setLeftCollapsed((collapsed) => !collapsed)}
        onKeyDown={(event) => handleResizeKey("left", event)}
        title="Drag to resize · double-click to collapse"
        aria-label="Resize workspaces panel"
        aria-orientation="vertical"
        aria-valuemin={MIN_PANEL_WIDTH}
        aria-valuemax={MAX_PANEL_WIDTH}
        aria-valuenow={leftWidth}
      />

      <main className="workspace-center-panel">
        {/* The tab strip is a plain row: the tablist is the scrollport itself,
            so the "New tab" button and its menu are siblings of the tablist,
            never descendants of it. */}
        <div className="workspace-session-tabs">
          {/* The row of tabs scrolls; the add button below it stays outside
              the scrollport, so a full strip cannot carry it off screen. */}
          <div
            className="workspace-session-tabs-scroll workspace-scroll"
            role="tablist"
            aria-label="Sessions"
            ref={stripScrollportRef}
          >
            {visibleSessions.map((session) => {
              const originBadge = sessionOriginBadge(session, peerNames);
              // A badge for a session the daemon described as a peer's, or the
              // unknown one for a session it did not describe at all. The two are
              // told apart by their words and by the mark on the unknown pill.
              const originUnknown = sessionOriginUnknown(session);
              // Identity badge: who created this session, in the same pill the
              // peer device uses. Null for a session a person started.
              // No `input_required` badge belongs here: the A2A task state is
              // reported to the creator (finish envelope + `child_finished`), not
              // to the roster, and the parked card a person must answer is what
              // the attention pill below already names. See
              // `workspaceSessions.ts` for the measurement before re-adding one.
              const creatorBadge = sessionCreatorBadge(session, sessionNames);
              // The delegation ledger, in pills: nothing for a session that is
              // not an agent-created child, the loud unattended pill for one that
              // asks nobody, the quiet answering pill for one whose creator
              // answers, the softer cannot-establish marker where the daemon
              // honestly could not.
              const delegationBadges = cachedDelegationBadges(session);
              // The take-back lives on the row that can act, beside its pill:
              // qualifying rows only, while the one switch is on.
              const rowTakeBack = takeBackAvailable && sessionDelegationTakeBack(session);
              const tabTitle = sessionTitle(session);
              return (
                // One row per session: the tab button, the trailing close
                // chip (a sibling — a button cannot live inside a button),
                // and the take-back. The right-click lives on the ROW, so a
                // right-click anywhere on it — over the chip included —
                // opens the tab menu.
                <div
                  className="workspace-session-row"
                  key={session.id}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    tabClose.openMenu(session.id);
                  }}
                >
                  <button
                    type="button"
                    role="tab"
                    id={sessionTabElementId(session.id)}
                    aria-selected={selectedSessionId === session.id}
                    aria-controls={WORKSPACE_TERMINAL_PANEL_ID}
                    aria-haspopup="menu"
                    aria-expanded={tabClose.menu?.sessionId === session.id}
                    className={`workspace-session-tab${selectedSessionId === session.id ? " workspace-session-tab-selected" : ""}${selection.has(session.id) ? " workspace-session-tab-multiselected" : ""}${session.attention ? " workspace-session-tab-attention" : ""}`}
                    onClick={(event) => handleTabClick(session, event)}
                    onAuxClick={(event) => {
                      // Paseo's middle click: button 1 closes the tab, by
                      // the same policy as every other close.
                      if (event.button === 1) {
                        event.preventDefault();
                        tabClose.closeSingle(session.id);
                      }
                    }}
                    onKeyDown={(event) => {
                      if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
                        event.preventDefault();
                        tabClose.openMenu(session.id);
                      }
                    }}
                  >
                    <span
                      className={`workspace-status-dot workspace-dot-${sessionDotTone(session.state)}`}
                    />
                    <span className="workspace-tab-label">{sessionTitle(session)}</span>
                    {originBadge !== null ? (
                      <span
                        className={
                          originUnknown
                            ? "workspace-session-origin-badge workspace-session-origin-badge-unknown"
                            : "workspace-session-origin-badge"
                        }
                        title={originBadge}
                      >
                        {originBadge}
                      </span>
                    ) : null}
                    {creatorBadge !== null ? (
                      <span className="workspace-session-origin-badge" title={creatorBadge}>
                        {creatorBadge}
                      </span>
                    ) : null}
                    {delegationBadges.map((badge) => (
                      <span
                        // Tone alone is not unique: two unknown-tone markers
                        // (a delegation ledger the daemon could not describe
                        // beside an unattended mode it could not establish)
                        // are exactly the row the honesty rules can produce.
                        key={`${badge.tone}:${badge.label}`}
                        className={`workspace-tab-delegation workspace-tab-delegation-${badge.tone}`}
                        title={badge.label}
                      >
                        {badge.label}
                      </span>
                    ))}
                    <span className="workspace-tab-meta">
                      {sessionStateLabel(session.state, session.elapsedMs)}
                    </span>
                    {session.attention ? (
                      <span
                        className={`workspace-tab-attention workspace-attention-${session.attention.reason}`}
                      >
                        {sessionAttentionLabel(session.attention.reason)}
                      </span>
                    ) : null}
                  </button>
                  <span className="workspace-session-chip">
                    <button
                      type="button"
                      className="workspace-session-chip-close"
                      aria-label={`Close ${tabTitle}`}
                      title={`Close ${tabTitle}`}
                      onClick={() => tabClose.closeSingle(session.id)}
                    >
                      <svg
                        width={12}
                        height={12}
                        viewBox="0 0 12 12"
                        aria-hidden="true"
                        focusable="false"
                      >
                        <path
                          d="M2 2l8 8M10 2l-8 8"
                          stroke="currentColor"
                          strokeWidth="1.5"
                          strokeLinecap="round"
                          fill="none"
                        />
                      </svg>
                    </button>
                  </span>
                  {rowTakeBack ? (
                    <button
                      type="button"
                      className="workspace-tab-takeback"
                      // A sibling of its tab, never a control inside one: the
                      // tab is a button, and a button cannot answer inside
                      // another. Global scope is the control's whole honesty —
                      // it takes back the power everywhere, not on this row.
                      aria-label="Take back — stops every agent from answering for its children"
                      title="Take back — stops every agent from answering for its children"
                      onClick={takeBack}
                    >
                      Take back
                    </button>
                  ) : null}
                </div>
              );
            })}
          </div>
          <div className="workspace-session-add-wrap">
            <button
              ref={addButtonRef}
              type="button"
              className="workspace-session-add"
              onClick={() => setNewTabMenuOpen((open) => !open)}
              title="New tab"
              aria-label="New tab"
              aria-haspopup="menu"
              aria-expanded={newTabMenuOpen}
              disabled={sessionCreating || providerChoosing}
            >
              +
            </button>
            {newTabMenuOpen ? (
              <WorkspaceNewTabMenu
                triggerRef={addButtonRef}
                creating={sessionCreating || providerChoosing}
                workspaceSelected={selectedWorkspace !== null}
                onAgent={handleNewTabAgent}
                onTerminal={handleNewTabTerminal}
                onClose={dismissNewTabMenu}
              />
            ) : null}
            {providerAnchor?.kind === "strip" ? providerMenu : null}
          </div>
          <span className="workspace-tabs-spacer" />
          <span className="workspace-rate">{sessionStatusText}</span>
        </div>
        <div className="workspace-sr-only" role="status" aria-live="polite">
          {selectionAnnouncement}
        </div>
        {tabClose.menu !== null ? (
          <SessionTabMenu
            anchorRef={tabClose.anchorRef}
            entries={tabClose.menu.entries}
            onEntry={tabClose.activateEntry}
            onClose={tabClose.closeMenu}
          />
        ) : null}
        {tabClose.confirm !== null ? (
          <CloseConfirm
            anchorRef={tabClose.anchorRef}
            title={tabClose.confirm.title}
            message={tabClose.confirm.message}
            confirmLabel={tabClose.confirm.confirmLabel}
            onConfirm={tabClose.confirmClose}
            onCancel={tabClose.cancelClose}
          />
        ) : null}
        <DaemonRestartNotice
          instanceId={daemon.instanceId}
          hasRecovered={sessions.some(isRecoveredSession)}
        />

        {closeFailures.length > 0 ? (
          // A close that did not go through — or a target that went stale
          // between the ask and the click — is named here, one line per
          // session. The store owns it: a later clean close or a
          // session_not_found for the same session and generation clears its
          // line. The heading stays neutral because the list can mix
          // archives with deletes; each line names its own verb.
          <div className="workspace-session-error" role="alert">
            <span className="workspace-session-error-text">
              These closes didn&apos;t go through:
            </span>
            {closeFailures.map((error) => (
              <span className="workspace-session-error-text" key={error.id}>
                {error.message}
              </span>
            ))}
            <button
              type="button"
              className="workspace-session-error-dismiss"
              onClick={() => closeActions.clearFailures()}
              aria-label="Dismiss error"
              title="Dismiss error"
            >
              ×
            </button>
          </div>
        ) : null}

        {sessionsError !== null ? (
          <div className="workspace-session-error" role="alert">
            <span className="workspace-session-error-text">{sessionsError}</span>
            <button
              type="button"
              className="workspace-session-error-dismiss"
              onClick={dismissSessionsError}
              aria-label="Dismiss error"
              title="Dismiss error"
            >
              ×
            </button>
          </div>
        ) : null}

        {delegationState.error !== null ? (
          // The refusal (or failed read) reported where the delegation control
          // lives — the roster row's take-back included — never only on the
          // Settings tab (audit 3, F4: a refused consent control may not be
          // silent on the surface it was clicked on). No dismiss button: the
          // sentence is the store's standing answer, and the next successful
          // read or write clears it.
          <div className="workspace-session-error" role="alert">
            <span className="workspace-session-error-text">{delegationState.error}</span>
          </div>
        ) : null}

        {selectedSessionId !== null ? (
          <>
            <RecoveredSessionBar
              session={selectedSession}
              onReopened={handleReopenSession}
              onResumeFailed={handleResumeFailed}
            />
            {selectedSession != null && isAgentKind(selectedSession.kind) ? (
              <AgentChatSurface
                key={selectedSessionId}
                id={WORKSPACE_TERMINAL_PANEL_ID}
                sessionId={selectedSessionId}
                title={sessionTitle(selectedSession)}
                cwd={selectedSession.cwd}
                observedState={selectedSession.state}
                elapsedMs={selectedSession.elapsedMs}
                daemonState={daemon.state}
                sessionRoster={sessions}
                deviceNames={peerNames}
                auxiliary={
                  selectedPermission !== null ? (
                    <WorkspacePermissionCard
                      key={selectedPermission.request.toolCallId}
                      sessionId={selectedSessionId}
                      subscriptionId={selectedPermission.subscriptionId}
                      request={selectedPermission.request}
                      capabilities={daemon.capabilities}
                      daemonState={daemon.state}
                      origin={selectedSession?.origin}
                      deviceNames={peerNames}
                      resolution={selectedPermission.resolution ?? null}
                      creatorId={selectedSession?.createdBy ?? null}
                      onResolved={dismissResolvedPermission}
                    />
                  ) : undefined
                }
                onPermissionRequest={handlePermissionRequest}
                onPermissionResolved={handlePermissionResolved}
              />
            ) : (
              <TerminalSurface
                key={selectedSessionId}
                id={WORKSPACE_TERMINAL_PANEL_ID}
                workspaceId={selectedWorkspace}
                sessionId={selectedSessionId}
                observedState={selectedSession?.state ?? null}
                cwd={selectedSession?.cwd}
                autoFocus={terminalAutoFocus}
                autoFocusGuard={mayTakeTerminalFocus}
                onAutoFocusTaken={takeTerminalFocus}
                onClosed={handleSessionClosed}
                onExited={handleSessionClosed}
                onPermissionRequest={handlePermissionRequest}
                onPermissionResolved={handlePermissionResolved}
              />
            )}
          </>
        ) : (
          <div
            id={WORKSPACE_TERMINAL_PANEL_ID}
            className="workspace-conversation workspace-scroll workspace-session-empty"
            role="tabpanel"
            aria-label="Terminal output"
          >
            <div role="status">
              {sessionsError ??
                (sessionsLoading
                  ? "Loading sessions…"
                  : "No tabs yet. Use + to open an agent or a terminal.")}
            </div>
          </div>
        )}
      </main>

      <button
        type="button"
        className="workspace-resize-handle"
        onMouseDown={(event) => startDrag("right", event)}
        onDoubleClick={() => setRightCollapsed((collapsed) => !collapsed)}
        onKeyDown={(event) => handleResizeKey("right", event)}
        title="Drag to resize · double-click to collapse"
        aria-label="Resize side panel"
        aria-orientation="vertical"
        aria-valuemin={MIN_PANEL_WIDTH}
        aria-valuemax={MAX_PANEL_WIDTH}
        aria-valuenow={rightWidth}
      />

      <aside
        className="workspace-panel workspace-right-panel"
        style={{ width: rightCollapsed ? "30px" : `${rightWidth}px` }}
        aria-label="Workspace side panel"
      >
        {rightCollapsed ? (
          <button
            type="button"
            className="workspace-collapsed-panel"
            onClick={() => setRightCollapsed(false)}
            title="Show side panel"
            aria-label="Show side panel"
          >
            <span aria-hidden="true">‹</span>
            <span className="workspace-vertical-label">side panel</span>
          </button>
        ) : (
          <div className="workspace-panel-open">
            <div className="workspace-right-toolbar">
              <button
                type="button"
                className="workspace-icon-button"
                onClick={() => setRightCollapsed(true)}
                title="Collapse"
                aria-label="Collapse side panel"
              >
                ›
              </button>
              <button
                type="button"
                className="workspace-surface-selector"
                onClick={() => setSurfaceMenuOpen((open) => !open)}
                aria-haspopup="listbox"
                aria-expanded={surfaceMenuOpen}
              >
                <span
                  className={`workspace-status-dot workspace-surface-dot-${selectedSurface.dotTone}`}
                />
                <span className="workspace-surface-name">{selectedSurface.name}</span>
                <span className="workspace-surface-meta">{surfaceMeta}</span>
                <span className="workspace-surface-chevron" aria-hidden="true">
                  ▾
                </span>
              </button>
            </div>

            {surfaceMenuOpen ? (
              <div
                className="workspace-surface-menu"
                role="listbox"
                aria-label="Show in this panel"
              >
                <div className="workspace-menu-label">Show in this panel</div>
                <div className="workspace-surface-options">
                  {sidePanelRegistry.map((surface) => (
                    <button
                      type="button"
                      role="option"
                      aria-selected={activeSidePanel === surface.id}
                      className={`workspace-surface-option${activeSidePanel === surface.id ? " workspace-surface-option-selected" : ""}`}
                      key={surface.id}
                      onClick={() => {
                        setActiveSidePanel(surface.id);
                        setSurfaceMenuOpen(false);
                      }}
                    >
                      <span
                        className={`workspace-status-dot workspace-surface-dot-${surface.dotTone}`}
                      />
                      <span className="workspace-surface-name">{surface.name}</span>
                      <span className="workspace-surface-option-meta">
                        {surface.liveMeta?.snapshot(selectedWorkspace) ?? surface.meta}
                      </span>
                    </button>
                  ))}
                </div>
              </div>
            ) : null}

            <div className="workspace-scroll workspace-side-scroll">
              {selectedSurface.render({
                appBuild,
                onReload: handleAppReload,
                prLabel,
                onOpenPullRequest: handleOpenPullRequest,
                workspaceId: selectedWorkspace,
              })}
            </div>
          </div>
        )}
      </aside>

      <NewProjectDialog
        open={projectDialogOpen}
        onClose={closeProjectDialog}
        onCreate={handleCreateProject}
      />
    </section>
  );
}
