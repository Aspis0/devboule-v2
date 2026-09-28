import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ChangeEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { ErrorText } from "../../components/ErrorText";
import { ConfirmProvider } from "../../components/ConfirmHost";
import { SurfaceErrorBoundary } from "../../app/SurfaceErrorBoundary";
import { NewProjectDialog } from "../../components/NewProjectDialog";
import { SIDE_PANEL_REGISTRY, type SidePanelEntry } from "./sidePanelRegistry";
import { SIDE_PANEL_BODY_ID, SidePanelTabs, sidePanelTabId } from "./panel/SidePanelTabs";
import { useMenuOpen } from "../../lib/menuOpen";
import { TerminalSurface } from "../terminal/TerminalSurface";
import { AgentChatSurface } from "./AgentChatSurface";
import { sharedSessionQueueOwner } from "./sessionQueueOwner";
import { useTabSelection } from "./strip/useTabSelection";
import { useTabCloseFlow } from "./strip/useTabCloseFlow";
import { useSessionRename } from "./strip/useSessionRename";
import { buildTabCloseEntries } from "./strip/tabCloseMenu";
import { SessionStrip } from "./strip/SessionStrip";
import { SessionRenameDialog } from "./strip/SessionRenameDialog";
import { discardPersistedPendingCloses, sharedCloseActions } from "./strip/closeActions";
import type { CloseIntent } from "./strip/closePolicy";
import { useWorkspaceDaemon } from "./workspaceDaemon";
import { reportSelection } from "./presence";
import { createDaemonRecovery } from "./daemonRecovery";
import {
  MAX_LEFT_WIDTH,
  MAX_RIGHT_WIDTH,
  MIN_LEFT_WIDTH,
  MIN_RIGHT_WIDTH,
  useWorkspacePanelResize,
} from "./workspaceResize";
import { useWorkspaceProjects } from "./workspaceProjects";
import { setLastSelectedWorkspaceId } from "./lastSelectedWorkspace";
import { Sidebar } from "./sidebar/Sidebar";
import { useWorkspaceStats } from "./sidebar/useWorkspaceStats";
import { useProviderConsent } from "./useProviderConsent";
import { focusIsWhereTheFlowLeftIt, useStripFocus } from "./strip/stripFocus";
import { AnchoredPopover } from "./popoverPlace";
import {
  DELEGATION_CAPABILITY,
  delegationController,
  type DelegationController,
} from "../../lib/delegation";
import { SESSION_RENAME_CAPABILITY } from "../../lib/sessionRename";
import {
  PermissionCard as WorkspacePermissionCard,
  formatPermissionCommand,
  resolutionOutcome,
} from "../../components/PermissionCard";
import {
  chatCapableProviders,
  peerDeviceNames,
  requiresConsent,
  sessionCreateFromProvider,
  sessionCreatorTooltip,
  sessionTitle,
  isRecoveredSession,
  sharedSessionController,
  useWorkspaceSessions,
} from "./workspaceSessions";
import {
  heldAssistantTextFor,
  setAttentionHeldContentProvider,
  workspaceHeldContentProvider,
} from "./attentionNotice";
import { RecoveredSessionBar } from "./recoveredSessionBar";
import { DaemonRestartNotice } from "./daemonRestartNotice";
import type { PermissionRequest, PermissionResolved, ProviderInfo, Session } from "../../types/ipc";
import { isAgentKind } from "../../types/ipc";
import {
  daemonRestart,
  devicesList,
  providersList,
  sessionClose,
  sessionStop,
} from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import "./Workspace.css";
import "./panel/panel.css";
import { useAppStore } from "../../store/appStore";

type ActiveSidePanel = SidePanelEntry["id"];
/**
 * Where the provider choice UI is anchored: a project's "New workspace" row
 * or the tab strip's session "+" button. It only decides placement; the
 * menu itself never reads which button opened it.
 */
type ProviderAnchor = { kind: "project"; projectId: string } | { kind: "strip" };
const WORKSPACE_TERMINAL_PANEL_ID = "workspace-panel-terminal";
/** The negotiated capability the Changes panel's history read is gated on. */
const WORKSPACE_GIT_LOG = "workspace.git_log";

export { WorkspacePermissionCard, formatPermissionCommand };

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
 * The session whose pane the centre may render: the selected id counts only
 * while the strip still has its tab. A row the strip hides (its close is in
 * flight, the roster carried it away) must never keep a pane up, and an empty
 * strip means the empty state. Since the strip is filtered to the selected
 * workspace's tabs (R2a's navigation rule), membership here is also workspace
 * isolation: another workspace's session cannot appear in the strip's list.
 */
export function paneSessionOf<S extends { id: string }>(
  selectedSessionId: string | null,
  stripSessions: readonly S[],
): S | null {
  return stripSessions.find((session) => session.id === selectedSessionId) ?? null;
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
  /** The daemon's own word for the option that was chosen, when it said one. */
  selectedOptionName: string | null;
}

export function Workspace({
  sidePanelRegistry = SIDE_PANEL_REGISTRY,
  delegation: delegationControllerProp,
}: WorkspaceProps = {}) {
  const delegation = delegationControllerProp ?? delegationController;
  const {
    projects,
    visibleProjects,
    loading: projectsLoading,
    error: projectsError,
    selectedWorkspace,
    setSelectedWorkspace,
    setSessionFacts,
    search,
    handleSearchChange,
    projectDialogOpen,
    openProjectDialog,
    closeProjectDialog,
    handleCreateProject,
    newProjectTriggerRef,
    retryProjects,
    reuseOrCreateWorkspace,
  } = useWorkspaceProjects();
  // The one seam Settings → Providers may use: the last-selected workspace,
  // read when a provider install/login opens its terminal tab. The surfaces
  // never mount together, so the cell outlives them; unmount clears nothing.
  useEffect(() => {
    setLastSelectedWorkspaceId(selectedWorkspace);
  }, [selectedWorkspace]);
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
  // What a toast may quote for a session — the pending permission card's
  // text and the last assistant message — is wired below, once the strip's
  // own rows exist: the provider's inputs are what this render puts on
  // screen, never a ref a later effect fills.
  const daemon = useWorkspaceDaemon();
  useEffect(() => {
    if (daemon.state !== "connected") sharedSessionQueueOwner().onDisconnect();
  }, [daemon.state]);
  // The empty provider picker's action hands the user to Settings → Providers
  // (the surface opens on that tab), so the flow needs the app's one switcher.
  const selectSurface = useAppStore((state) => state.selectSurface);
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

  const sidebarWorkspaceIds = useMemo(
    () => visibleProjects.flatMap((project) => project.workspaces.map((w) => w.id)),
    [visibleProjects],
  );
  const endedKey = useMemo(
    () =>
      sessions
        .filter((session) => session.state.type === "ended")
        .map((session) => session.id)
        .join("\n"),
    [sessions],
  );
  const { stats: workspaceStats, refresh: refreshWorkspaceStats } = useWorkspaceStats(
    sidebarWorkspaceIds,
    {
      connected: daemon.state === "connected",
      selectedWorkspace,
      endedKey,
    },
  );
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
      // An explicit close or archive takes the queued messages with the
      // session: the journal keeps the row, so the roster would go on naming a
      // session the user just removed and its queue would never be dropped by
      // the absence rule alone (`sessionQueueOwner.ts::closeSession`).
      archive: (id) =>
        sessionStop(id).then(() => {
          sharedSessionQueueOwner().closeSession(id);
        }),
      destroy: (id) =>
        sessionClose(id).then(() => {
          sharedSessionQueueOwner().closeSession(id);
        }),
    }),
  );
  const knownWorkspaceIds = useMemo(
    // All listed projects, not the search-filtered view: search hiding a
    // workspace's row must not veto navigation into it.
    () => new Set(projects.flatMap((project) => project.workspaces.map((w) => w.id))),
    [projects],
  );

  const closingIds = useSyncExternalStore(closeActions.subscribe, closeActions.getClosingSnapshot);
  const closeFailures = useSyncExternalStore(
    closeActions.subscribe,
    closeActions.getFailuresSnapshot,
  );
  const visibleSessions = useMemo(() => {
    const hiding = new Set(closingIds);
    // Selection is navigation (Paseo's rule): the strip shows only the
    // selected workspace's tabs, so an empty workspace shows the empty state
    // instead of another workspace's tabs.
    // A session with no workspace (a legacy record) has no home to navigate
    // to, so it renders in every strip; hiding it would make it unreachable.
    return sessions.filter(
      (session) =>
        !hiding.has(session.id) &&
        (session.workspaceId === selectedWorkspace || session.workspaceId === null),
    );
  }, [sessions, closingIds, selectedWorkspace]);
  // What a toast may quote for a session: the pending permission card's text
  // and the last assistant message, and only for a row this window's tab
  // strip actually renders. The provider is rebuilt from the rendered rows
  // and the queue as they are now, and asks the close marks per call — a row
  // hidden by an in-flight close is not "visible in this window" even before
  // the daemon removes it from the roster. With the strip scoped to the
  // selected workspace, "rendered" means rendered there; it decides WORDING
  // only, never whether the raise announces (the toast gate asks the looked-at
  // session for that, Paseo's rule).
  const renderedSessionIds = useMemo(
    () => new Set(visibleSessions.map((session) => session.id)),
    [visibleSessions],
  );
  useEffect(() => {
    setAttentionHeldContentProvider(
      workspaceHeldContentProvider({
        rendered: (sessionId) =>
          renderedSessionIds.has(sessionId) &&
          !closeActions.getClosingSnapshot().includes(sessionId),
        pending: (sessionId) =>
          permissionQueue.find(
            (item) => item.sessionId === sessionId && item.resolution === undefined,
          )?.request,
        heldAssistantText: heldAssistantTextFor,
      }),
    );
    return () => setAttentionHeldContentProvider(null);
  }, [renderedSessionIds, permissionQueue, closeActions]);
  // Selection is navigation (Paseo), reconciled in ONE effect from ONE
  // snapshot so workspace and session can never undo each other across
  // renders (two effects here once fought: one scheduled the workspace
  // switch while the other, still closing over the old strip, pulled the
  // session back — the next render reversed both and could loop). A restored
  // or pushed selection that lives in another listed workspace selects that
  // workspace and keeps the session — until the user has navigated by row
  // click once, after which their clicks alone steer the view (a create that
  // lands after a switch must not yank it back). Every other way of losing
  // the selected session from the selected workspace's strip falls to that
  // workspace's first tab or the empty state.
  useEffect(() => {
    if (!userNavigatedRef.current) {
      const selected = sessions.find((session) => session.id === selectedSessionId);
      if (
        selected !== undefined &&
        selected.workspaceId !== null &&
        selected.workspaceId !== selectedWorkspace &&
        knownWorkspaceIds.has(selected.workspaceId)
      ) {
        setSelectedWorkspace(selected.workspaceId);
        return;
      }
    }
    if (visibleSessions.some((session) => session.id === selectedSessionId)) return;
    selectSession(visibleSessions[0]?.id ?? null);
  }, [
    sessions,
    selectedSessionId,
    selectedWorkspace,
    knownWorkspaceIds,
    visibleSessions,
    setSelectedWorkspace,
    selectSession,
  ]);
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
      // Fired when the close is REQUESTED, before the acts dispatch, and it
      // runs for closes that later fail too — a failed close simply re-reads
      // the same numbers. The daemon sends no stop transition, so without
      // this read `+N −M` would lag until the 30-second cadence. The stats
      // refresh is deliberately NOT a roster refresh: the roster still
      // reports the rows live, and refreshing would unmark the closes and
      // resurrect them.
      const closedWorkspaceIds = new Set(
        matched
          .map((session) => sessions.find((roster) => roster.id === session.id)?.workspaceId)
          .filter((workspaceId) => workspaceId !== null && workspaceId !== undefined),
      );
      if (closedWorkspaceIds.size > 0) refreshWorkspaceStats([...closedWorkspaceIds]);
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
    [closeActions, refreshWorkspaceStats, sessions],
  );
  // Multi-select and the tab close flow live in the strip's folder; the
  // strip only wires their handlers. The "+" button's ref is the flow's
  // last focus fallback (no active tab).
  const addButtonRef = useRef<HTMLButtonElement>(null);
  const tabSelection = useTabSelection({
    sessions: visibleSessions,
    selectedSessionId,
    selectSession,
  });
  const renameSupported = daemon.capabilities.includes(SESSION_RENAME_CAPABILITY);
  const rename = useSessionRename({ sessions: visibleSessions, renameSupported });
  const tabClose = useTabCloseFlow({
    sessions: visibleSessions,
    selectedSessionId,
    selection: tabSelection.selection,
    onClose: runClose,
    selectSession,
    clearSelection: tabSelection.clearSelection,
    addButtonRef,
    renameMenu: { entriesFor: rename.renameEntriesFor, open: rename.openRename },
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
  // The centre pane exists only while the strip still has the selected tab:
  // a row the strip hides (its close is in flight, the roster carried it away)
  // or another workspace's session must never keep a pane up — an empty strip
  // means the empty state.
  const paneSession = paneSessionOf(selectedSessionId, visibleSessions);
  // The queue the pane's session drains into belongs to the app, not to this
  // surface: the owner holds one per session for the whole run, so opening
  // Settings, switching tabs or a refresh that rebuilds the strip cannot
  // destroy a message the user queued (review F1, F2, F13).
  const sessionQueue =
    paneSession === null ? null : sharedSessionQueueOwner().queueFor(paneSession.id);
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
    // A reconnect may carry a restored selection from the journal: the view
    // must honour it again, so the user's earlier row click no longer stands
    // down automatic navigation. If the user's workspace was removed in the
    // same transition, the validity effect has already moved the selection;
    // a stuck flag would keep showing that stale view.
    userNavigatedRef.current = false;
    void retryProjects();
    void reconnectSessions();
    void refreshPeerNames();
  }, [daemon.state, reconnectSessions, refreshPeerNames, retryProjects]);
  // The presence reporter itself is App's (one per app run, wherever the user is
  // standing); this surface only owns the fact of what it shows. That fact is
  // written from the STORE, not from a render: the selection is decided inside
  // the controller — a row click, a roster push, the reconcile below — and a
  // roster push arriving in the next task is judged against it. Reporting from
  // an effect keyed to `selectedSessionId` runs a commit later than the publish,
  // and the toast gate would hold a raise back for the session the user already
  // left. Leaving the surface (Settings, Design) withdraws the record, so neither
  // the daemon nor the local gate keeps excusing a session nobody shows.
  useLayoutEffect(() => {
    const controller = sharedSessionController();
    let reported = controller.getState().selectedSessionId;
    reportSelection(reported);
    const unsubscribe = controller.subscribe(() => {
      const next = controller.getState().selectedSessionId;
      if (next === reported) return;
      reported = next;
      reportSelection(next);
    });
    return () => {
      unsubscribe();
      reportSelection(null);
    };
  }, []);
  const handleReopenSession = useCallback(
    (session: Session) => {
      openSession(session);
      // Selection is navigation: reopening a History session moves the view
      // to the workspace that session lives in.
      if (session.workspaceId !== null) setSelectedWorkspace(session.workspaceId);
      setHistoryOpen(false);
      setHistorySearch("");
    },
    [openSession, setSelectedWorkspace],
  );
  // A failed resume leaves the row's verdict changed on the daemon side; the
  // bar must not keep its offer on the roster data this surface already held.
  const handleResumeFailed = useCallback(() => {
    void refreshSessions();
  }, [refreshSessions]);
  const [providerPicker, setProviderPicker] = useState<ProviderInfo[] | null>(null);
  const [providerAnchor, setProviderAnchor] = useState<ProviderAnchor | null>(null);
  /** Set on the first row-click navigation: afterwards the user steers the
   * view, and automatic selection-to-workspace navigation stands down. */
  const userNavigatedRef = useRef(false);
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
  const [providerError, setProviderError] = useState<ErrorSentence | null>(null);
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
      // Defect (a): the project's local workspace is reused — selected, then
      // the agent spawns there — and minted only when the project has none,
      // so "+" stops producing look-alike "devboule-v2" rows. Distinct
      // workspaces arrive with the worktree slice (R2b).
      const workspace = await reuseOrCreateWorkspace(projectId);
      if (workspace !== null) startAgentSession(provider, workspace.id);
      endProviderChoice();
    },
    [endProviderChoice, reuseOrCreateWorkspace, startAgentSession],
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
        setProviderError(errorSentence(cause));
        return;
      }
      if (capable.length === 0) {
        // Gate before create (the recon's Paseo reading, §5a): with no
        // chat-capable provider the create would be born doomed, so the flow
        // stops here — the anchored picker opens with its empty state and
        // afterChoice is never called. The choice ends through the picker's
        // own dismissal, or the button that opens the install guidance.
        afterProviderChoiceRef.current = afterChoice;
        providerAnchorElRef.current = trigger ?? addButtonRef.current;
        setProviderAnchor(anchor);
        setProviderPicker([]);
        return;
      }
      if (capable.length === 1 && !requiresConsent(capable[0])) {
        // The in-flight guard must hold until the afterChoice callback has
        // fully settled: the callback releases it (defect: releasing here let
        // a second click mint a second workspace while the first create was
        // still running).
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
  const handleResizeStart = useCallback(
    (event: ReactMouseEvent<HTMLButtonElement>) => startDrag("left", event),
    [startDrag],
  );
  const handleResizeKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLButtonElement>) => handleResizeKey("left", event),
    [handleResizeKey],
  );
  const handleToggleHistory = useCallback(() => setHistoryOpen((open) => !open), []);
  const handleHistorySearchChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setHistorySearch(event.target.value),
    [],
  );
  const handleRetryProjects = useCallback(() => void retryProjects(), [retryProjects]);
  const selectWorkspace = useCallback(
    (workspaceId: string) => {
      userNavigatedRef.current = true;
      setSelectedWorkspace(workspaceId);
      // The session selection moves with the navigation: the workspace's
      // first tab, or none (its empty state).
      selectSession(sessions.find((session) => session.workspaceId === workspaceId)?.id ?? null);
    },
    [selectSession, sessions, setSelectedWorkspace],
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
      sessionsError: sessionsError?.sentence ?? null,
      providerError: providerError?.sentence ?? null,
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
  // The provider choice (and its consent gate) dismisses when the band
  // opens, like every other menu: the flow is transient and the band is
  // the outside press.
  useMenuOpen(providerPicker !== null || consentProvider !== null, dismissPickerFlow);
  // The empty picker's one action. Settings opens on its Providers tab, where
  // the install guidance lives; dismissing first ends the choice so the
  // navigate-away can never leave a flow running under the user left behind.
  const openProvidersSettings = useCallback(() => {
    dismissPickerFlow();
    selectSurface("settings");
  }, [dismissPickerFlow, selectSurface]);
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
        const selectedOptionName = resolution.selectedOptionName?.trim() || null;
        const next = [...queue];
        next[index] = { ...next[index], resolution: { outcome, answeredBy, selectedOptionName } };
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
  // An unanswered card parks the turn: the chat surface turns Enter's queue
  // action into a steer while one is open (queueing would strand the message).
  const hasPendingPermission =
    selectedPermission !== null && selectedPermission.resolution === undefined;
  // The creator lookup the strip's tooltips resolve against: one map per
  // roster, so a chip never scans the roster for its own row.
  const creatorById = useMemo(() => new Map(sessions.map((row) => [row.id, row])), [sessions]);
  // Stable across renders of the same roster, so the strip's per-row memo
  // below only recomputes when its inputs change.
  const resolveCreator = useCallback(
    (session: Session) => sessionCreatorTooltip(session, creatorById),
    [creatorById],
  );
  // The strip's status slot carries progress and the count, never an error
  // text: a failure has its own one line (the spec's inline error line), so
  // the slot never becomes its second, third and fourth surface.
  const sessionStatusText = sessionCreating
    ? "Starting session…"
    : sessionsLoading && sessions.length === 0
      ? "Loading sessions…"
      : // The count reads the strip, not the roster: a session the strip has
        // dropped (its close succeeded) is not a session on screen. The
        // daemon still owes a roster push after session_stop (a recorded
        // daemon item); runClose refreshes the closed sessions' stats, not
        // the roster — a roster refresh would resurrect the closed rows.
        `${visibleSessions.length} session${visibleSessions.length === 1 ? "" : "s"}`;

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
            {providerPicker!.length === 0 ? (
              // The gate's empty state: no agent CLI is installed, so the flow
              // stops here instead of creating a session that cannot start.
              <div className="workspace-provider-empty">
                <p className="workspace-provider-empty-text">
                  No agent CLI is installed on this machine. Install one — for example grok, claude,
                  or gemini — then choose Refresh in Settings → Providers.
                </p>
                <button
                  type="button"
                  className="workspace-empty-action"
                  onClick={openProvidersSettings}
                >
                  Install instructions
                </button>
              </div>
            ) : (
              [
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
                ))
            )}
          </>
        )}
      </AnchoredPopover>
    );

  return (
    <section className="workspace-screen" data-screen-label="Workspace">
      <Sidebar
        width={leftWidth}
        collapsed={leftCollapsed}
        onCollapsedChange={setLeftCollapsed}
        onResizeStart={handleResizeStart}
        onResizeKeyDown={handleResizeKeyDown}
        resizeMin={MIN_LEFT_WIDTH}
        resizeMax={MAX_LEFT_WIDTH}
        historyOpen={historyOpen}
        onToggleHistory={handleToggleHistory}
        history={{
          searchValue: historySearch,
          onSearchChange: handleHistorySearchChange,
          onReopen: handleReopenSession,
        }}
        searchValue={search}
        onSearchChange={handleSearchChange}
        onAddProject={openProjectDialog}
        addProjectRef={newProjectTriggerRef}
        daemon={daemon}
        daemonNote={restartFailureNote}
        tree={{
          projects: visibleProjects,
          loading: projectsLoading,
          error: projectsError,
          providerError,
          selectedWorkspace,
          onRetryProjects: handleRetryProjects,
          onSelectWorkspace: selectWorkspace,
          onNewWorkspace: handleNewWorkspace,
          providerMenuAnchorProjectId:
            providerAnchor?.kind === "project" ? providerAnchor.projectId : null,
          providerMenu: providerAnchor?.kind === "project" ? providerMenu : null,
          stats: workspaceStats,
        }}
      />

      <main className="workspace-center-panel">
        <SessionStrip
          sessions={visibleSessions}
          selectedSessionId={selectedSessionId}
          selectSession={selectSession}
          tabSelection={tabSelection}
          tabClose={tabClose}
          addButtonRef={addButtonRef}
          newTab={{
            open: newTabMenuOpen,
            creating: sessionCreating || providerChoosing,
            workspaceSelected: selectedWorkspace !== null,
            onToggle: () => setNewTabMenuOpen((open) => !open),
            onAgent: handleNewTabAgent,
            onTerminal: handleNewTabTerminal,
            onCloseMenu: dismissNewTabMenu,
          }}
          providerMenu={providerAnchor?.kind === "strip" ? providerMenu : null}
          peerNames={peerNames}
          resolveCreator={resolveCreator}
          takeBackAvailable={takeBackAvailable}
          onTakeBack={takeBack}
          statusText={sessionStatusText}
        />
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
            {closeFailures.map((failure) => (
              <span
                className="workspace-session-error-text"
                key={failure.id}
                title={failure.detail ?? undefined}
                aria-describedby={
                  failure.detail !== null ? `close-failure-${failure.id}-detail` : undefined
                }
              >
                {failure.message}
                {failure.detail !== null ? (
                  <span id={`close-failure-${failure.id}-detail`} className="error-detail-sr-only">
                    {failure.detail}
                  </span>
                ) : null}
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

        {sessionsError !== null &&
        (sessionsError.workspaceId === null || sessionsError.workspaceId === selectedWorkspace) ? (
          // The one render of the create/list failure: the spec's inline error
          // line (12, --danger, triangle), shown over the workspace the
          // failure belongs to. The daemon's own words ride in the tooltip
          // and the described-by node — never painted beside the sentence.
          <div
            className="workspace-error-line"
            role="alert"
            title={sessionsError.detail ?? undefined}
            aria-describedby={
              sessionsError.detail !== null ? "workspace-session-error-detail" : undefined
            }
          >
            <svg
              className="workspace-error-line-icon"
              viewBox="0 0 12 12"
              aria-hidden="true"
              focusable="false"
            >
              <path d="M6 1.6 11 10.4H1Z" />
            </svg>
            <span className="workspace-error-line-text">{sessionsError.sentence}</span>
            {sessionsError.detail !== null ? (
              <span id="workspace-session-error-detail" className="error-detail-sr-only">
                {sessionsError.detail}
              </span>
            ) : null}
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
            <span className="workspace-session-error-text">
              <ErrorText
                sentence={delegationState.error.sentence}
                detail={delegationState.error.detail}
                id="workspace-delegation-error"
              />
            </span>
          </div>
        ) : null}

        {paneSession !== null ? (
          <>
            <RecoveredSessionBar
              session={paneSession}
              onReopened={handleReopenSession}
              onResumeFailed={handleResumeFailed}
            />
            {isAgentKind(paneSession.kind) ? (
              <AgentChatSurface
                key={paneSession.id}
                id={WORKSPACE_TERMINAL_PANEL_ID}
                sessionId={paneSession.id}
                title={sessionTitle(paneSession)}
                cwd={paneSession.cwd}
                observedState={paneSession.state}
                elapsedMs={paneSession.elapsedMs}
                activity={paneSession.activity}
                attention={paneSession.attention}
                daemonState={daemon.state}
                sessionRoster={sessions}
                headerMenuSeam={{
                  closeEntries: buildTabCloseEntries(
                    visibleSessions.findIndex((row) => row.id === paneSession.id),
                    visibleSessions.length,
                  ),
                  onCloseEntry: (key) => tabClose.activatePaneEntry(paneSession.id, key),
                  // Two gates, both daemon-side: the terminal's seam carries
                  // no rename, and a journal-replayed (recovered) session is
                  // refused by the daemon's one rename road (it reaches the
                  // record only through a live process). An ended-but-live
                  // session renames fine.
                  onRename:
                    renameSupported && paneSession.state.type !== "recovered"
                      ? () => rename.openRename(paneSession.id)
                      : null,
                }}
                deviceNames={peerNames}
                hasPendingPermission={hasPendingPermission}
                // The app-level owner's queue for this session (see
                // `sessionQueue`): the surface binds its controller to it, and
                // Enter mid-turn queues instead of interrupting (review P1-1).
                queue={sessionQueue}
                auxiliary={
                  selectedPermission !== null ? (
                    <WorkspacePermissionCard
                      key={selectedPermission.request.toolCallId}
                      sessionId={paneSession.id}
                      subscriptionId={selectedPermission.subscriptionId}
                      request={selectedPermission.request}
                      capabilities={daemon.capabilities}
                      daemonState={daemon.state}
                      origin={paneSession.origin}
                      deviceNames={peerNames}
                      resolution={selectedPermission.resolution ?? null}
                      creatorId={paneSession.createdBy ?? null}
                      onResolved={dismissResolvedPermission}
                    />
                  ) : undefined
                }
                onPermissionRequest={handlePermissionRequest}
                onPermissionResolved={handlePermissionResolved}
              />
            ) : (
              <TerminalSurface
                key={paneSession.id}
                id={WORKSPACE_TERMINAL_PANEL_ID}
                workspaceId={selectedWorkspace}
                sessionId={paneSession.id}
                observedState={paneSession.state}
                cwd={paneSession.cwd}
                activity={paneSession.activity}
                attention={paneSession.attention}
                autoFocus={terminalAutoFocus}
                autoFocusGuard={mayTakeTerminalFocus}
                onAutoFocusTaken={takeTerminalFocus}
                onClosed={handleSessionClosed}
                onExited={handleSessionClosed}
                onCloseTab={() => tabClose.closeSingle(paneSession.id)}
                headerMenuSeam={{
                  closeEntries: buildTabCloseEntries(
                    visibleSessions.findIndex((row) => row.id === paneSession.id),
                    visibleSessions.length,
                  ),
                  onCloseEntry: (key) => tabClose.activatePaneEntry(paneSession.id, key),
                }}
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
            {/* The empty state never carries the error: the failure has its
                one line under the strip, and this pane stays what the spec
                says it is (SPEC-regions "Empty and error"). */}
            {sessionsLoading ? (
              <div role="status" className="workspace-empty-note">
                Loading sessions…
              </div>
            ) : (
              <div className="workspace-empty-state" role="status">
                <p className="workspace-empty-title">No tabs yet</p>
                {/* The spec's one outline action: the same agent flow as
                    "+ → Agent", from the "+" itself. */}
                <button
                  type="button"
                  className="workspace-empty-action"
                  onClick={handleNewTabAgent}
                >
                  Open an agent
                </button>
              </div>
            )}
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
        aria-valuemin={MIN_RIGHT_WIDTH}
        aria-valuemax={MAX_RIGHT_WIDTH}
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
            autoFocus
            onClick={() => setRightCollapsed(false)}
            title="Show side panel"
            aria-label="Show side panel"
          >
            <span aria-hidden="true">‹</span>
            <span className="workspace-vertical-label">side panel</span>
          </button>
        ) : (
          <div className="workspace-panel-open">
            <SidePanelTabs
              registry={sidePanelRegistry}
              activeId={activeSidePanel}
              onSelect={setActiveSidePanel}
              onCollapse={() => setRightCollapsed(true)}
            />

            <div
              key={`${selectedSurface.id}:${selectedWorkspace ?? ""}`}
              id={SIDE_PANEL_BODY_ID}
              className="workspace-scroll workspace-side-scroll"
              // A kebab body is menu-opened, not tab-associated: a named
              // region keeps the tablist's one-selected-tab invariant (APG).
              role={selectedSurface.placement === "tab" ? "tabpanel" : "region"}
              aria-label={selectedSurface.name}
              aria-labelledby={
                selectedSurface.placement === "tab" ? sidePanelTabId(selectedSurface.id) : undefined
              }
              // APG tabs: the panel joins the tab sequence only when it has
              // no focusable content of its own (the honest empty panels).
              tabIndex={selectedSurface.placement === "tab" ? -1 : 0}
            >
              {/* A body throw replaces the body only; the tab row above stays
                    mounted so the user can leave the panel. The scrollport
                    carries the key, so a workspace or panel switch remounts
                    body and offset together: drafts, menus, errors and scroll
                    all start clean. */}
              <SurfaceErrorBoundary surfaceLabel={selectedSurface.name}>
                {/* The destructive asks' host: inside the keyed body, so a
                    panel or workspace switch unmounts it and declines a
                    standing ask instead of acting behind the new panel. */}
                <ConfirmProvider>
                  {selectedSurface.render({
                    workspaceId: selectedWorkspace,
                    // The one fact the Changes panel gates on, computed from
                    // the daemon status this component already holds — a
                    // panel reads it here instead of polling for its own.
                    canListCommits:
                      daemon.state === "connected" &&
                      daemon.capabilities.includes(WORKSPACE_GIT_LOG),
                  })}
                </ConfirmProvider>
              </SurfaceErrorBoundary>
            </div>
          </div>
        )}
      </aside>

      <NewProjectDialog
        open={projectDialogOpen}
        onClose={closeProjectDialog}
        onCreate={handleCreateProject}
      />

      {/* Rendered last of the two: both backdrops share z-index 50 and both
          dialogs are always mounted, so the tie is declaration order — this
          one sits on top regardless of which opened first. */}
      <SessionRenameDialog rename={rename.rename} onClose={rename.closeRename} />
    </section>
  );
}
