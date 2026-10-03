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
import { pendingPlanId } from "./pendingPlanId";
import {
  composeStripTabs,
  isToolTabId,
  makeBrowserTab,
  makeToolTab,
  openToolTabs,
  pruneToolTabsForWorkspaces,
  type FileToolTabKind,
  type ToolTab,
} from "./strip/toolTabs";
import { ToolDiffPane } from "./ToolDiffPane";
import { WorkspaceFileTab } from "./WorkspaceFileTab";
import { BrowserTab } from "./BrowserTab";
import {
  activeBrowserTabFor,
  browserLayoutSnapshot,
  openBrowserTab,
  pruneBrowserTabs,
  routeBrowserPopup,
  subscribeBrowserLayout,
} from "./browserTabs";
import { normalizeBrowserUrl, BROWSER_START_URL } from "./browserUrl";
import { closeBrowserPage } from "./browserPages";
import { ErrorTriangleIcon } from "./ErrorTriangleIcon";
import { createToolContentCache, evictToolContent } from "./toolContentCache";
import { localWorkspaceKey, parseWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import { activeTabFor, forgetTab, pruneTabMemory, rememberActiveTab } from "./workspaceTabMemory";
import { useTabSelection } from "./strip/useTabSelection";
import { useTabCloseFlow } from "./strip/useTabCloseFlow";
import { useSessionRename } from "./strip/useSessionRename";
import { buildTabCloseEntries } from "./strip/tabCloseMenu";
import {
  activeSessionAttention,
  sessionAttentionLabel,
  sessionNeedsApproval,
} from "./sessionAttention";
import { SessionStrip } from "./strip/SessionStrip";
import { SessionRenameDialog } from "./strip/SessionRenameDialog";
import { discardPersistedPendingCloses, sharedCloseActions } from "./strip/closeActions";
import type { CloseIntent } from "./strip/closePolicy";
import { usePairedDevices, useWorkspaceDaemon } from "./workspaceDaemon";
import { reportSelection } from "./presence";
import { createDaemonRecovery } from "./daemonRecovery";
import {
  MAX_LEFT_WIDTH,
  MAX_RIGHT_WIDTH,
  MIN_LEFT_WIDTH,
  MIN_RIGHT_WIDTH,
  useWorkspacePanelResize,
} from "./workspaceResize";
import { keyOfWorkspace, useWorkspaceProjects } from "./workspaceProjects";
import { getLastSelectedWorkspaceKey, setLastSelectedWorkspaceKey } from "./lastSelectedWorkspace";
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
import { isImeComposition } from "../../lib/imeComposition";
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
import type {
  PermissionRequest,
  PermissionResolved,
  ProviderInfo,
  Session,
  SessionKind,
} from "../../types/ipc";
import { isAgentKind } from "../../types/ipc";
import { daemonRestart, providersList, sessionClose, sessionStop } from "../../lib/tauri";
import { isCommandError } from "../../lib/commandError";
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
/** The negotiated capability the shared follow-up queue is gated on: the
 * daemon holds the rows, and this workspace asks it for them. */
const SESSION_QUEUE_CAPABILITY = "session.queue";
/** The negotiated capability GIF and WebP attachments are gated on. */
const ATTACHMENTS_GIF_WEBP_CAPABILITY = "attachments.gif_webp";

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
 * Which strips a session's tab belongs to. A session with no workspace — the
 * Design surface creates those — belongs to every one of them, so its tab
 * stays reachable wherever the user navigates. A tab takes its host from the
 * workspace it belongs to; the roster carries no host of its own.
 */
function belongsToWorkspaceKey(session: Session, key: WorkspaceKey | null): boolean {
  return session.workspaceId === null || localWorkspaceKey(session.workspaceId) === key;
}

/**
 * The workspace the view is about to move to, or null when it stays. The
 * roster names one (`pushed`) by selecting a session in it, and the view
 * follows that name until the user has taken the wheel with a row click —
 * or until the workspace in force is the one this mount restored, which is
 * the user's own last navigation and outlives the surface that recorded it.
 * Both roads that answer a selection the strip cannot show read this: the one
 * that moves the view and the one that picks the tab it lands on.
 */
function workspaceTheViewMovesTo(
  pushed: WorkspaceKey | null,
  userNavigated: boolean,
  restoredWorkspaceKey: WorkspaceKey | null,
  selectedKey: WorkspaceKey | null,
): WorkspaceKey | null {
  if (pushed === null || userNavigated) return null;
  if (restoredWorkspaceKey !== null && restoredWorkspaceKey === selectedKey) return null;
  return pushed;
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

interface QueuedPermission {
  sessionId: string;
  subscriptionId: number;
  request: PermissionRequest;
  /** Set when an agent answered this card elsewhere; the card stays to say so. */
  resolution?: QueueResolution;
  /** Set when answering failed terminally; the card keeps only Clear. */
  stale?: boolean;
}

export function Workspace({
  sidePanelRegistry = SIDE_PANEL_REGISTRY,
  delegation: delegationControllerProp,
}: WorkspaceProps = {}) {
  const delegation = delegationControllerProp ?? delegationController;
  // Captured once per mount, not read on every render: the cell moves with the
  // selection, so a mount that re-read it would take its own navigation for
  // the user's. App keys the surface boundary by surface, so a visit to
  // Settings or Design remounts this component, and the workspace the user was
  // standing in outlives the surface that recorded it.
  const [restoredWorkspaceKey] = useState(getLastSelectedWorkspaceKey);
  const {
    projects,
    visibleProjects,
    loading: projectsLoading,
    error: projectsError,
    selectedKey,
    setSelectedKey,
    setSessionFacts,
    search,
    handleSearchChange,
    projectDialogOpen,
    openProjectDialog,
    closeProjectDialog,
    handleCreateProject,
    newProjectTriggerRef,
    retryProjects,
    renameWorkspace,
    deleteWorkspace,
    reuseOrCreateWorkspace,
  } = useWorkspaceProjects(restoredWorkspaceKey);
  // The one seam Settings → Providers may use: the last-selected workspace,
  // read when a provider install/login opens its terminal tab. It is also what
  // the next mount of this surface starts on. The surfaces never mount
  // together, so the cell outlives them; unmount clears nothing.
  useEffect(() => {
    setLastSelectedWorkspaceKey(selectedKey);
  }, [selectedKey]);
  // What the daemon is sent: the selection's own id, never its key.
  const selectedWorkspaceId =
    selectedKey === null ? null : parseWorkspaceKey(selectedKey).workspaceId;
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
  const [permissionQueue, setPermissionQueue] = useState<QueuedPermission[]>([]);
  // Drop one session's cards. The close acts call this on success and on a
  // moot close, and the roster absence rule below calls it when the row is
  // gone: a failed close while the session still exists keeps them.
  const dropSessionPermissions = (id: string) => {
    setPermissionQueue((queue) => queue.filter((item) => item.sessionId !== id));
  };
  // What a toast may quote for a session — the pending permission card's
  // text and the last assistant message — is wired below, once the strip's
  // own rows exist: the provider's inputs are what this render puts on
  // screen, never a ref a later effect fills.
  const daemon = useWorkspaceDaemon();
  const devices = usePairedDevices();
  // Device id to display name, for the tab badge that names a peer session's
  // device. The sidebar's host list reads the same payload on the same poll, so
  // the names are derived from that read rather than from a second one.
  const peerNames = useMemo(() => peerDeviceNames(devices.peers), [devices]);
  const queueSupported = daemon.capabilities.includes(SESSION_QUEUE_CAPABILITY);
  const gifWebpSupported = daemon.capabilities.includes(ATTACHMENTS_GIF_WEBP_CAPABILITY);
  // The empty provider picker's action hands the user to Settings → Providers
  // (the surface opens on that tab), so the flow needs the app's one switcher.
  const selectSurface = useAppStore((state) => state.selectSurface);
  const {
    sessions,
    openSessions,
    selectedSessionId,
    loading: sessionsLoading,
    creating: sessionCreating,
    error: sessionsError,
    refresh: refreshSessions,
    reconnect: reconnectSessions,
    create: createSession,
    select: selectSession,
    open: openSession,
    closeTabs: closeSessionTabsIn,
    dismissError: dismissSessionsError,
  } = useWorkspaceSessions(selectedWorkspaceId);
  // Every road that takes a session's tab out of the strip lands here, so a
  // closed id leaves every workspace's memory — an unscoped session's tab
  // belongs to all of them, and an entry left behind elsewhere would restore
  // it the next time the same id came back.
  const closeSessionTabs = useCallback(
    (ids: readonly string[]) => {
      for (const id of ids) forgetTab(id);
      closeSessionTabsIn(ids);
    },
    [closeSessionTabsIn],
  );
  // Tool tabs live beside the sessions; the last session stays selected
  // underneath an active tool so the reconcile below keeps passing.
  const [toolTabs, setToolTabs] = useState<ToolTab[]>([]);
  const [activeToolTabId, setActiveToolTabId] = useState<string | null>(null);
  // What the tool tabs last showed, so a tab switched away from and back to
  // re-reads over its old content instead of an empty cell. Per mount, never
  // shared: each pane seeds from it and writes its landed reads back.
  const [toolContentCache] = useState(createToolContentCache);

  const sidebarWorkspaceKeys = useMemo(
    () =>
      visibleProjects
        .flatMap((project) => project.workspaces.map(keyOfWorkspace))
        .filter((key) => key !== null),
    [visibleProjects],
  );
  const sidebarKeySet = useMemo(() => new Set(sidebarWorkspaceKeys), [sidebarWorkspaceKeys]);
  // History's rows join the visible tree's sweep while it is open, so a row
  // the tree search hides, or no project lists, still gets its branch.
  const [historyWorkspaceKeys, setHistoryWorkspaceKeys] = useState<readonly WorkspaceKey[]>([]);
  const handleHistoryWorkspaceKeys = useCallback((keys: readonly WorkspaceKey[]) => {
    setHistoryWorkspaceKeys((current) =>
      current.join("\u0000") === keys.join("\u0000") ? current : keys,
    );
  }, []);
  const statsWorkspaceKeys = useMemo(
    () => [...new Set([...sidebarWorkspaceKeys, ...historyWorkspaceKeys])].sort(),
    [sidebarWorkspaceKeys, historyWorkspaceKeys],
  );
  const endedKey = useMemo(() => {
    const ended: string[] = [];
    for (const session of sessions) {
      if (session.state.type !== "ended" || session.workspaceId === null) continue;
      const key = localWorkspaceKey(session.workspaceId);
      if (key !== null && sidebarKeySet.has(key)) ended.push(session.id);
    }
    return ended.join("\n");
  }, [sessions, sidebarKeySet]);
  const {
    stats: workspaceStats,
    branches: workspaceBranches,
    refresh: refreshWorkspaceStats,
    evict: evictWorkspaceStats,
  } = useWorkspaceStats(statsWorkspaceKeys, {
    connected: daemon.state === "connected",
    selectedKey,
    endedKey,
  });
  const handleDeleteWorkspace = useCallback(
    async (workspaceId: string): Promise<ErrorSentence | null> => {
      const refusal = await deleteWorkspace(workspaceId);
      // The daemon's row is gone; History can keep naming the id for its
      // sessions, so the cache is dropped here and never waits for History.
      const key = localWorkspaceKey(workspaceId);
      if (refusal === null && key !== null) evictWorkspaceStats(key);
      return refusal;
    },
    [deleteWorkspace, evictWorkspaceStats],
  );
  useEffect(() => {
    setSessionFacts(sessions);
  }, [sessions, setSessionFacts]);
  // Tab membership cannot discard cards; roster absence or an ended process can.
  const seenSessionIdsRef = useRef<Set<string>>(new Set());
  useEffect(() => {
    for (const session of sessions) seenSessionIdsRef.current.add(session.id);
    const present = new Set(
      sessions.filter((session) => session.state.type !== "ended").map((session) => session.id),
    );
    setPermissionQueue((queue) => {
      if (
        !queue.some(
          (item) => seenSessionIdsRef.current.has(item.sessionId) && !present.has(item.sessionId),
        )
      )
        return queue;
      return queue.filter(
        (item) => !seenSessionIdsRef.current.has(item.sessionId) || present.has(item.sessionId),
      );
    });
  }, [sessions]);
  // The strip's close acts: fire at once (the undo window is gone), hide the
  // row until the roster confirms, and own each failure by the act that
  // produced it. App-lifetime, like the acts themselves: a fire still in the
  // air when the user switches to Settings lands its error here, and the
  // list is still shown when the Workspace mounts again.
  const [closeActions] = useState(() =>
    sharedCloseActions({
      // An explicit close or archive takes the session's permission cards with
      // it. A close that fails while the session still exists keeps them; a
      // moot one (`session_not_found`: the row is already gone) drops them and
      // lets the store stay silent. The queue needs nothing here: the daemon
      // fences and clears it with the session, and publishes the empty queue.
      archive: (id) =>
        sessionStop(id).then(
          () => {
            dropSessionPermissions(id);
          },
          (cause: unknown) => {
            if (isCommandError(cause) && cause.code === "session_not_found") {
              dropSessionPermissions(id);
            }
            throw cause;
          },
        ),
      destroy: (id) =>
        sessionClose(id).then(
          () => {
            dropSessionPermissions(id);
          },
          (cause: unknown) => {
            if (isCommandError(cause) && cause.code === "session_not_found") {
              dropSessionPermissions(id);
            }
            throw cause;
          },
        ),
    }),
  );
  const knownWorkspaceKeys = useMemo(
    // All listed projects, not the search-filtered view: search hiding a
    // workspace's row must not veto navigation into it.
    () =>
      new Set(
        projects
          .flatMap((project) => project.workspaces.map(keyOfWorkspace))
          .filter((key) => key !== null),
      ),
    [projects],
  );
  // Keyed on contents, never Set identity, so roster pushes skip the prune.
  const workspaceKeyText = useMemo(
    () => [...knownWorkspaceKeys].sort().join("\n"),
    [knownWorkspaceKeys],
  );
  // Pruning runs as an effect on the workspace set's CONTENTS, never during
  // render: `pruneBrowserTabs` publishes to every browser-layout listener and
  // writes localStorage, and neither may happen while this renders.
  const prunedWorkspaceKeyText = useRef(workspaceKeyText);
  useEffect(() => {
    if (prunedWorkspaceKeyText.current === workspaceKeyText) return;
    prunedWorkspaceKeyText.current = workspaceKeyText;
    pruneTabMemory(knownWorkspaceKeys);
    setToolTabs((prev) => {
      for (const tab of prev) {
        if (knownWorkspaceKeys.has(tab.workspaceKey)) continue;
        // A browser tab has no path to evict; its page is disposed of below,
        // where the ids the prune dropped come back out.
        if (tab.kind !== "browser") evictToolContent(toolContentCache, tab.workspaceKey, tab.path);
      }
      return pruneToolTabsForWorkspaces(prev, knownWorkspaceKeys);
    });
    for (const browserId of pruneBrowserTabs(knownWorkspaceKeys)) closeBrowserPage(browserId);
  }, [knownWorkspaceKeys, toolContentCache, workspaceKeyText]);

  const closingIds = useSyncExternalStore(closeActions.subscribe, closeActions.getClosingSnapshot);
  const closeFailures = useSyncExternalStore(
    closeActions.subscribe,
    closeActions.getFailuresSnapshot,
  );
  // One workspace predicate serves both the strip and overview so navigation cannot drift.
  const inSelectedWorkspace = useCallback(
    (session: Session) => belongsToWorkspaceKey(session, selectedKey),
    [selectedKey],
  );
  const visibleSessions = useMemo(() => {
    const hiding = new Set(closingIds);
    return openSessions.filter(
      (session) => !hiding.has(session.id) && inSelectedWorkspace(session),
    );
  }, [openSessions, closingIds, inSelectedWorkspace]);
  const browserLayout = useSyncExternalStore(subscribeBrowserLayout, browserLayoutSnapshot);
  const visibleToolTabs = useMemo(() => {
    const own = toolTabs.filter((tab) => tab.workspaceKey === selectedKey);
    if (selectedKey === null) return own;
    // A browser tab's strip entry is derived from the tab model rather than
    // held here: the model is what persists it, so a second copy would be a
    // second answer to "which browser tabs does this workspace have".
    return [
      ...own,
      ...browserLayout.tabs
        .filter((tab) => tab.workspaceKey === selectedKey)
        .map((tab) => makeBrowserTab(selectedKey, tab.browserId)),
    ];
  }, [toolTabs, selectedKey, browserLayout.tabs]);
  /** The tab a workspace lands on when no session of its own comes first. */
  const browserTabIdFor = useCallback((key: WorkspaceKey): string | null => {
    const browserId = activeBrowserTabFor(key);
    return browserId === null ? null : makeBrowserTab(key, browserId).id;
  }, []);
  // The tab ids another workspace would render: the live set a remembered
  // tab has to still belong to, or the switch falls back. Built per call
  // because it is only ever asked for a workspace being entered.
  const liveTabIdsFor = useCallback(
    (key: WorkspaceKey): ReadonlySet<string> => {
      const hiding = new Set(closingIds);
      const ids = new Set<string>();
      for (const session of openSessions) {
        if (!hiding.has(session.id) && belongsToWorkspaceKey(session, key)) ids.add(session.id);
      }
      for (const tab of toolTabs) {
        if (tab.workspaceKey === key) ids.add(tab.id);
      }
      for (const tab of browserLayout.tabs) {
        if (tab.workspaceKey === key) ids.add(makeBrowserTab(key, tab.browserId).id);
      }
      return ids;
    },
    [browserLayout.tabs, closingIds, openSessions, toolTabs],
  );
  // Where a workspace lands when it is entered: the tab it was last showing,
  // if that tab is still live, else the fallback the switch has always used —
  // the workspace's first open session, or none. The row click and the mount
  // both ask here, so a fallback written twice cannot drift.
  const landingTabFor = useCallback(
    (key: WorkspaceKey): string | null =>
      activeTabFor(
        key,
        liveTabIdsFor(key),
        openSessions.find(
          (session) =>
            session.workspaceId !== null && localWorkspaceKey(session.workspaceId) === key,
        )?.id ?? browserTabIdFor(key),
      ),
    [browserTabIdFor, liveTabIdsFor, openSessions],
  );
  const openEndedIds = useMemo(
    () => new Set(openSessions.filter((row) => row.state.type === "ended").map((row) => row.id)),
    [openSessions],
  );
  // Ended sessions belong to History unless the user already has their tab open.
  // Durable membership keeps them reachable during a pending lifecycle close.
  const overviewSessions = useMemo(
    () =>
      sessions.filter(
        (row) =>
          inSelectedWorkspace(row) &&
          (isRecoveredSession(row) ||
            row.state.type === "live" ||
            row.state.type === "silent" ||
            openEndedIds.has(row.id)),
      ),
    [sessions, inSelectedWorkspace, openEndedIds],
  );
  const workspaceName = useMemo(
    () =>
      projects
        .flatMap((project) => project.workspaces)
        .find((w) => keyOfWorkspace(w) === selectedKey)?.title ?? null,
    [projects, selectedKey],
  );
  const activeTool = visibleToolTabs.find((tab) => tab.id === activeToolTabId) ?? null;
  /** A browser tab's restored page, so the pane opens where the tab was. */
  const browserRecordFor = useCallback(
    (browserId: string) =>
      browserLayout.tabs.find((record) => record.browserId === browserId) ?? null,
    [browserLayout.tabs],
  );
  /** Where a browser tab's page is, for the copy row that offers an address
   * instead of the path a file tab has. */
  const browserTabAddress = useCallback(
    (browserId: string) => browserRecordFor(browserId)?.url ?? null,
    [browserRecordFor],
  );
  const activeToolId = activeTool?.id ?? null;
  const activeTabId = activeToolId ?? selectedSessionId;
  const composedTabs = useMemo(
    () => composeStripTabs(visibleSessions, visibleToolTabs),
    [visibleSessions, visibleToolTabs],
  );
  // The one writer of the per-workspace tab memory, because there are many
  // roads to a selection and only one answer to "what is on screen now": a
  // chip, the Overview, a History reopen, the roster reconcile below, a tool
  // pane. A tab that is not in this workspace's strip is a frame between two
  // workspaces, and an empty selection over a strip that still has tabs is
  // the same frame — neither is recorded, so neither can file one workspace's
  // tab under another's key.
  useEffect(() => {
    if (selectedKey === null) return;
    if (activeTabId === null) {
      if (composedTabs.length === 0) rememberActiveTab(selectedKey, null);
      return;
    }
    if (composedTabs.some((tab) => tab.id === activeTabId))
      rememberActiveTab(selectedKey, activeTabId);
  }, [activeTabId, composedTabs, selectedKey]);
  // What a toast may quote for a session: the pending permission card's text
  // and the last assistant message, and only for a row this window's tab
  // strip actually renders. The provider is rebuilt from the rendered rows
  // and the queue as they are now, and asks the close marks per call — a row
  // hidden by an in-flight close is not "visible in this window" even before
  // the daemon removes it from the roster. With the strip scoped to the
  // selected workspace, "rendered" means rendered there; it decides WORDING
  // only, never whether the raise announces (the toast gate asks the
  // looked-at session for that).
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
  // Where a selection this workspace's strip cannot show says the view belongs:
  // a session in another listed workspace is the roster naming a view, not a
  // leftover. Null means nothing is asking for a move. The reconcile below and
  // the restore after it read this, so the road that moves the view and the
  // road that picks the tab cannot disagree about what is pushing.
  const pushedWorkspaceKey = useMemo(() => {
    const selected = sessions.find((session) => session.id === selectedSessionId);
    if (selected === undefined || selected.workspaceId === null) return null;
    const key = localWorkspaceKey(selected.workspaceId);
    if (key === null || key === selectedKey || !knownWorkspaceKeys.has(key)) return null;
    return key;
  }, [knownWorkspaceKeys, selectedSessionId, selectedKey, sessions]);
  // Selection is navigation, reconciled in ONE effect from ONE
  // snapshot so workspace and session can never undo each other across
  // renders (two effects here once fought: one scheduled the workspace
  // switch while the other, still closing over the old strip, pulled the
  // session back — the next render reversed both and could loop). A restored
  // or pushed selection that lives in another listed workspace selects that
  // workspace and keeps the session — until the user has navigated by row
  // click once, after which their clicks alone steer the view (a create that
  // lands after a switch must not yank it back). workspaceTheViewMovesTo owns
  // the rest of that rule. Every other way of losing the selected session from
  // the selected workspace's strip falls to that workspace's first tab or the
  // empty state.
  useEffect(() => {
    if (sessionsLoading || projectsLoading) return;
    const movesTo = workspaceTheViewMovesTo(
      pushedWorkspaceKey,
      userNavigatedRef.current,
      restoredWorkspaceKey,
      selectedKey,
    );
    if (movesTo !== null) {
      setSelectedKey(movesTo);
      return;
    }
    if (visibleSessions.some((session) => session.id === selectedSessionId)) return;
    selectSession(visibleSessions[0]?.id ?? null);
  }, [
    projectsLoading,
    pushedWorkspaceKey,
    restoredWorkspaceKey,
    selectedSessionId,
    selectedKey,
    sessionsLoading,
    visibleSessions,
    setSelectedKey,
    selectSession,
  ]);
  // One close, one act: the flow has already asked where the policy says so
  // and resolved its targets; what lands here fires now, and a refused act
  // names itself back to the flow (its row came back).
  const runClose = useCallback(
    (
      kind: CloseIntent,
      matched: readonly Session[],
      onFailed?: (sessionId: string) => void,
    ): void => {
      // Fired when the close is REQUESTED, before the acts dispatch, and it
      // runs for closes that later fail too — a failed close simply re-reads
      // the same numbers. The daemon sends no stop transition, so without
      // this read `+N −M` would lag until the 30-second cadence. The stats
      // refresh is deliberately NOT a roster refresh: the roster still
      // reports the rows live, and refreshing would unmark the closes and
      // resurrect them.
      const closedWorkspaceKeys = new Set<WorkspaceKey>();
      for (const session of matched) {
        const closedIn = sessions.find((roster) => roster.id === session.id)?.workspaceId;
        if (closedIn === null || closedIn === undefined) continue;
        const key = localWorkspaceKey(closedIn);
        if (key !== null) closedWorkspaceKeys.add(key);
      }
      if (closedWorkspaceKeys.size > 0) refreshWorkspaceStats([...closedWorkspaceKeys]);
      closeSessionTabs(matched.map((session) => session.id));
      for (const session of matched) {
        closeActions.act(
          kind,
          {
            id: session.id,
            title: sessionTitle(session),
            generation: session.state.generation,
          },
          () => {
            const selected = sharedSessionController().getState().selectedSessionId;
            openSession(session);
            selectSession(selected);
            onFailed?.(session.id);
          },
        );
      }
    },
    [closeActions, closeSessionTabs, openSession, selectSession, refreshWorkspaceStats, sessions],
  );
  // Multi-select and the tab close flow live in the strip's folder; the
  // strip only wires their handlers. The "+" button's ref is the flow's
  // last focus fallback (no active tab).
  const addButtonRef = useRef<HTMLButtonElement>(null);
  // A click away and back leaves activeToolTabId where it started — the value
  // alone cannot see it; the counted writes bump this move counter.
  const toolNavRef = useRef(0);
  const writeToolTab = useCallback((id: string | null) => {
    toolNavRef.current += 1;
    setActiveToolTabId(id);
  }, []);
  // Opening the same (kind, workspace, path) again focuses the existing
  // tab instead of duplicating it; opening focuses either way.
  const openToolTab = useCallback((key: WorkspaceKey, path: string, kind: FileToolTabKind) => {
    // One tab mints one id: the strip selects what it is given, never a
    // second string rebuilt from the same parts.
    const tab = makeToolTab(kind, key, path);
    setToolTabs((prev) => openToolTabs(prev, tab));
    setActiveToolTabId(tab.id);
  }, []);
  // A browser tab in a workspace, focused: the "+" entry and a page asking
  // for a window both land here, so a popup can never open as anything other
  // than the tab its owner asked for. An address that does not normalise is
  // dropped to the start page rather than refused — Rust gated it already.
  const openBrowserFor = useCallback((key: WorkspaceKey, url?: string) => {
    const record = openBrowserTab(key, url === undefined ? null : normalizeBrowserUrl(url));
    setActiveToolTabId(makeBrowserTab(key, record.browserId).id);
  }, []);
  // A page's window lands in the workspace that page's tab belongs to, never
  // in whichever workspace happens to be in front when it asks.
  useEffect(
    () =>
      routeBrowserPopup((sourceId, url) => {
        const owner = browserLayout.tabs.find((tab) => tab.browserId === sourceId);
        if (owner !== undefined) openBrowserFor(owner.workspaceKey, url);
      }),
    [browserLayout.tabs, openBrowserFor],
  );
  // Every stand-down routes through here. The landed create is the only caller
  // that may skip it (guarded below); the roster reconcile never calls it.
  const standDownToolTab = useCallback(() => writeToolTab(null), [writeToolTab]);
  // A click on the already-active tool tab re-reads it: the pane keeps the
  // old content until the new read lands (each pane owns that), so this is
  // only the nudge, never a remount.
  const [toolRefreshNonce, setToolRefreshNonce] = useState(0);
  // Mirrored after commit — layout, not passive, so a landed create can
  // never read a stale tool tab — and never read during render.
  const activeToolTabIdRef = useRef<string | null>(null);
  useLayoutEffect(() => {
    activeToolTabIdRef.current = activeToolTabId;
  });
  // The ONE selection write the strip-facing readers use: a tool id parks
  // beside the session authority, a session id clears it. A stale tool id
  // resolves to no visible tab, so the session underneath shows instead.
  const selectTab = useCallback(
    (id: string | null) => {
      if (id !== null && isToolTabId(id)) {
        if (id === activeToolTabIdRef.current) setToolRefreshNonce((nonce) => nonce + 1);
        writeToolTab(id);
        return;
      }
      standDownToolTab();
      selectSession(id);
    },
    [selectSession, standDownToolTab, writeToolTab],
  );
  // The roads that set the selected workspace WITHOUT choosing a tab for it:
  // the workspace this mount restores, a project list that no longer holds the
  // selection, a project row whose workspace the "+" reuses. Each lands on the
  // tab the workspace was last showing, while that tab is still one of its
  // own. It waits for the roster, because a remembered tab can only be tested
  // against a workspace's tabs once they are all there. It follows the
  // reconcile above, whose write would otherwise land on top of this one.
  useEffect(() => {
    if (sessionsLoading || projectsLoading) return;
    // The view is about to carry off to the workspace the roster named: the
    // reconcile above moves there and keeps that session, so no tab is chosen
    // for the workspace being left behind.
    if (
      workspaceTheViewMovesTo(
        pushedWorkspaceKey,
        userNavigatedRef.current,
        restoredWorkspaceKey,
        selectedKey,
      ) !== null
    ) {
      return;
    }
    if (selectedKey === null) return;
    // The tab this workspace was left on, while that tab is still one of its
    // own. What is showing stands as the answer to everything else: the
    // roster picks a tab before the app has asked, and the one the user
    // picked is already filed by the writer above.
    const remembered = activeTabFor(selectedKey, liveTabIdsFor(selectedKey), activeTabId);
    if (remembered === null || remembered === activeTabId) return;
    selectTab(remembered);
  }, [
    activeTabId,
    liveTabIdsFor,
    projectsLoading,
    pushedWorkspaceKey,
    restoredWorkspaceKey,
    selectTab,
    selectedKey,
    sessionsLoading,
  ]);
  // A landed create takes the pane only while the tool axis stood still: the
  // same tab as at start and no counted move in between. A move keeps the pane.
  const createAndShowSession = useCallback(
    (kind: SessionKind, provider: string | null, workspaceId: string | null) => {
      const toolTabAtStart = activeToolTabIdRef.current;
      const toolNavAtStart = toolNavRef.current;
      return createSession(kind, provider, workspaceId).then((session) => {
        if (
          session !== null &&
          toolTabAtStart !== null &&
          activeToolTabIdRef.current === toolTabAtStart &&
          toolNavRef.current === toolNavAtStart
        ) {
          standDownToolTab();
        }
        return session;
      });
    },
    [createSession, standDownToolTab],
  );
  const closeToolTabs = useCallback(
    (ids: readonly string[]): void => {
      if (ids.length === 0) return;
      const gone = new Set(ids);
      // A browser tab's page is a child webview in the Rust process: closing
      // its strip tab is what disposes it, and its record goes with it.
      for (const record of browserLayout.tabs) {
        if (!gone.has(makeBrowserTab(record.workspaceKey, record.browserId).id)) continue;
        closeBrowserPage(record.browserId);
      }
      setToolTabs((prev) => {
        for (const tab of prev) {
          if (tab.kind !== "browser" && gone.has(tab.id)) {
            evictToolContent(toolContentCache, tab.workspaceKey, tab.path);
          }
        }
        return prev.filter((tab) => !gone.has(tab.id));
      });
    },
    [browserLayout.tabs, toolContentCache],
  );
  const tabSelection = useTabSelection({
    tabs: composedTabs,
    activeTabId,
    selectTab,
  });
  const renameSupported = daemon.capabilities.includes(SESSION_RENAME_CAPABILITY);
  const rename = useSessionRename({ sessions: visibleSessions, renameSupported });
  const tabClose = useTabCloseFlow({
    sessions: visibleSessions,
    tabs: composedTabs,
    activeTabId,
    selection: tabSelection.selection,
    onClose: runClose,
    onCloseTabs: closeSessionTabs,
    onCloseTools: closeToolTabs,
    selectTab,
    clearSelection: tabSelection.clearSelection,
    addButtonRef,
    resolveBrowserAddress: browserTabAddress,
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
  // The centre shows the tool tab while one is active; the session pane
  // only stands when no tool covers it — its queue, permission card and
  // header menu unmount with it, so none of them act on the hidden session.
  const paneSession =
    activeTool !== null ? null : paneSessionOf(selectedSessionId, visibleSessions);
  // Primitive dependencies keep fileLinks stable across roster pushes so
  // unchanged messages retain their Markdown memo.
  // The pane's workspace, as the UI names it: the header menu reads it for the
  // branch row, the file links open tabs under it.
  const paneWorkspaceId = paneSession?.workspaceId ?? null;
  const paneWorkspaceKey = paneWorkspaceId === null ? null : localWorkspaceKey(paneWorkspaceId);
  const paneCwd = paneSession?.cwd;
  const chatFileLinks = useMemo(() => {
    if (paneWorkspaceId === null || paneCwd === undefined || paneWorkspaceKey === null) return null;
    return {
      root: paneCwd,
      open: (relativePath: string) => openToolTab(paneWorkspaceKey, relativePath, "file"),
    };
  }, [paneWorkspaceId, paneCwd, paneWorkspaceKey, openToolTab]);
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
  }, [daemon.state, reconnectSessions, retryProjects]);
  // The presence reporter itself is App's (one per app run, wherever the user is
  // standing); this surface only owns the fact of what it shows. That fact is
  // written from the STORE, not from a render: the selection is decided inside
  // the controller — a row click, a roster push, the reconcile below — and a
  // roster push arriving in the next task is judged against it. Reporting from
  // an effect keyed to `selectedSessionId` runs a commit later than the publish,
  // and the toast gate would hold a raise back for the session the user already
  // left. Leaving the surface (Settings, Design) withdraws the record, so neither
  // the daemon nor the local gate keeps excusing a session nobody shows.
  // While a tool tab is active the daemon is told no session is focused:
  // the person reads a diff, and neither presence nor the toast gate may
  // excuse the hidden session. Reselecting a session reports it again.
  // The subscription below is app-lifetime: it is set up once and reads the
  // active tool through a ref, so a tool switch never churns the subscription.
  const activeToolIdRef = useRef(activeToolId);
  useEffect(() => {
    activeToolIdRef.current = activeToolId;
  });
  useLayoutEffect(() => {
    const controller = sharedSessionController();
    const current = (): string | null =>
      activeToolIdRef.current !== null ? null : controller.getState().selectedSessionId;
    let reported = current();
    reportSelection(reported);
    const unsubscribe = controller.subscribe(() => {
      const next = current();
      if (next === reported) return;
      reported = next;
      reportSelection(next);
    });
    return () => {
      unsubscribe();
      reportSelection(null);
    };
  }, []);
  // A tool switch publishes nothing to the controller, so the switch reports
  // itself — the real value, never a transient.
  useLayoutEffect(() => {
    reportSelection(
      activeToolId !== null ? null : sharedSessionController().getState().selectedSessionId,
    );
  }, [activeToolId]);
  // Null under a tool tab is the decision, not an accident: the person is
  // not looking at the hidden session, so its raises still announce.
  const handleReopenSession = useCallback(
    (session: Session) => {
      // Reopening names the session to show: the tool tab stands down.
      standDownToolTab();
      openSession(session);
      const key = session.workspaceId === null ? null : localWorkspaceKey(session.workspaceId);
      if (key !== null) setSelectedKey(key);
      setHistoryOpen(false);
      setHistorySearch("");
    },
    [openSession, setSelectedKey, standDownToolTab],
  );
  const handleReopenAgent = useCallback(
    (session: Session) => {
      const latest = sharedSessionController()
        .getState()
        .sessions.find((row) => row.id === session.id);
      handleReopenSession(latest ?? session);
    },
    [handleReopenSession],
  );
  const handleOpenOverviewSession = useCallback(
    (sessionId: string) => {
      const session = sessions.find((row) => row.id === sessionId);
      if (session === undefined) return;
      // The overview's explicit open: the tool tab stands down and the
      // existing open path tabs and selects the session. No workspace
      // switch: every non-null overview row already lives in the selected
      // workspace, and a legacy null-workspace row has no home to go to.
      standDownToolTab();
      openSession(session);
    },
    [sessions, openSession, standDownToolTab],
  );
  const handleOpenSubagent = useCallback(
    (sessionId: string) => {
      const session = sessions.find((row) => row.id === sessionId);
      if (session !== undefined) {
        handleReopenSession(session);
      } else {
        void refreshSessions();
      }
    },
    [sessions, handleReopenSession, refreshSessions],
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
  const choiceWorkspaceRef = useRef<WorkspaceKey | null>(selectedKey);
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
      void createAndShowSession(args.kind, args.provider, workspaceId);
    },
    [createAndShowSession],
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
      startAgentSession(provider, selectedWorkspaceId);
      endProviderChoice();
    },
    [endProviderChoice, selectedWorkspaceId, startAgentSession],
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
      choiceWorkspaceRef.current = selectedKey;
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
        // Gate before create: with no chat-capable provider the create would
        // be born doomed, so the flow stops here — the anchored picker opens
        // with its empty state and afterChoice is never called. The choice
        // ends through the picker's own dismissal, or the button that opens
        // the install guidance.
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
    [endProviderChoice, loadChatProviders, requestConsent, selectedKey],
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
    (key: WorkspaceKey) => {
      // The row already in force is not navigation: no workspace changes, so
      // nothing is chosen either — which keeps it from reading the tool pane
      // again, a nudge that belongs to a click on the tab itself.
      if (key === selectedKey) return;
      userNavigatedRef.current = true;
      setSelectedKey(key);
      // The selection moves with the navigation: the tab this workspace was
      // last left on, else the fallback the switch has always used — the
      // workspace's first tab, or none (its empty state). Routing through
      // selectTab leaves the old workspace's tool tab behind with it.
      selectTab(landingTabFor(key));
    },
    [landingTabFor, selectTab, selectedKey, setSelectedKey],
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
      workspaceKey: selectedKey,
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
    void createAndShowSession("terminal", null, selectedWorkspaceId).then((session) => {
      if (session !== null) armTerminalFocus(session.id, selectedKey);
    });
  }, [armTerminalFocus, createAndShowSession, dismissNewTabMenu, selectedKey, selectedWorkspaceId]);
  // The menu's Browser entry: a page in the selected workspace, focused. It
  // creates no daemon session, so it does not wait for the strip's focus rule
  // — the address bar takes focus itself once the page is up.
  const handleNewTabBrowser = useCallback(() => {
    dismissNewTabMenu();
    if (selectedKey !== null) openBrowserFor(selectedKey);
  }, [dismissNewTabMenu, openBrowserFor, selectedKey]);
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
    if (choiceWorkspaceRef.current !== selectedKey) dismissPickerFlow();
  }, [providerChoosing, selectedKey, dismissPickerFlow]);
  useEffect(() => {
    if (providerAnchor === null && consentProvider === null) return;
    const onKey = (event: KeyboardEvent) => {
      if (isImeComposition(event)) return;
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
        // a dead id. Staleness rides along: the item is replaced, not reset.
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
  // A card whose answer the daemon refused terminally: the item records it,
  // so every remount starts terminal instead of offering the answer again.
  const markPermissionStale = useCallback((sessionId: string, toolCallId: string) => {
    setPermissionQueue((queue) => {
      const index = queue.findIndex(
        (item) =>
          item.sessionId === sessionId &&
          item.request.toolCallId === toolCallId &&
          item.stale !== true,
      );
      if (index === -1) return queue;
      const next = [...queue];
      next[index] = { ...next[index], stale: true };
      return next;
    });
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
  // mount: a daemon restart — even one the 2 s poll never saw
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
  // The honest gate for the control that stops delegation: it
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
  // hide a waiting one behind it — a resolved card offers only
  // Clear, so find-on-head made the waiting card's Allow/Deny unreachable
  // and said nothing about a second card existing. When nothing waits, the
  // resolved card stays on screen: it never vanishes on the strength of the
  // resolution event alone, and Clear is its removal path.
  // While a tool tab is active its pane owns the centre: the hidden
  // session's card has no surface to render on.
  const selectedPermission =
    activeTool !== null
      ? null
      : (permissionQueue.find(
          (item) => item.sessionId === selectedSessionId && item.resolution === undefined,
        ) ??
        permissionQueue.find((item) => item.sessionId === selectedSessionId) ??
        null);
  // An unanswered card parks the turn: the chat surface turns Enter's queue
  // action into a steer while one is open (queueing would strand the message).
  const hasPendingPermission =
    selectedPermission !== null && selectedPermission.resolution === undefined;
  // The one plan row that may stand down: the id of the card this pane
  // actually renders, while it waits unanswered. A plan whose card waits
  // behind another card keeps its row — a hidden plan with no card on screen
  // is worse than a duplicate.
  const pendingPlanToolCallId = pendingPlanId(selectedPermission);
  const subagentAttention = useMemo(
    () =>
      new Map(
        sessions.flatMap((row) => {
          if (!sessionNeedsApproval(row)) return [];
          const label = sessionAttentionLabel(row);
          return label === null ? [] : [[row.id, label] as const];
        }),
      ),
    [sessions],
  );
  // Cache creator lookup so each chip does not scan the roster.
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
      : `${visibleSessions.length} open session${visibleSessions.length === 1 ? "" : "s"}`;

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
          projects,
          branches: workspaceBranches,
          onWorkspaceKeysChange: handleHistoryWorkspaceKeys,
          selectedSessionId: activeToolId === null ? selectedSessionId : null,
          onSearchChange: handleHistorySearchChange,
          onReopen: handleReopenSession,
          onReopenAgent: handleReopenAgent,
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
          selectedWorkspace: selectedKey,
          onRetryProjects: handleRetryProjects,
          onSelectWorkspace: selectWorkspace,
          onNewWorkspace: handleNewWorkspace,
          onRenameWorkspace: renameWorkspace,
          onDeleteWorkspace: handleDeleteWorkspace,
          providerMenuAnchorProjectId:
            providerAnchor?.kind === "project" ? providerAnchor.projectId : null,
          providerMenu: providerAnchor?.kind === "project" ? providerMenu : null,
          stats: workspaceStats,
          branches: workspaceBranches,
        }}
      />

      <main className="workspace-center-panel">
        <SessionStrip
          tabs={composedTabs}
          activeTabId={activeTabId}
          selectTab={selectTab}
          tabSelection={tabSelection}
          tabClose={tabClose}
          addButtonRef={addButtonRef}
          newTab={{
            open: newTabMenuOpen,
            creating: sessionCreating || providerChoosing,
            workspaceSelected: selectedKey !== null,
            onToggle: () => setNewTabMenuOpen((open) => !open),
            onAgent: handleNewTabAgent,
            onTerminal: handleNewTabTerminal,
            onBrowser: handleNewTabBrowser,
            onCloseMenu: dismissNewTabMenu,
          }}
          providerMenu={providerAnchor?.kind === "strip" ? providerMenu : null}
          peerNames={peerNames}
          resolveCreator={resolveCreator}
          takeBackAvailable={takeBackAvailable}
          onTakeBack={takeBack}
          statusText={sessionStatusText}
          overviewSessions={overviewSessions}
          workspaceName={workspaceName}
          onOpenSession={handleOpenOverviewSession}
          selectedSessionId={selectedSessionId}
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
        (sessionsError.workspaceKey === null || sessionsError.workspaceKey === selectedKey) ? (
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
            <ErrorTriangleIcon />
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
          // Settings tab (a refused consent control may not be
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

        {activeTool?.kind === "browser" ? (
          <BrowserTab
            key={activeTool.id}
            browserId={activeTool.browserId}
            url={browserRecordFor(activeTool.browserId)?.url ?? BROWSER_START_URL}
          />
        ) : activeTool !== null ? (
          <div
            id={WORKSPACE_TERMINAL_PANEL_ID}
            className={`workspace-conversation workspace-scroll workspace-tool-pane${
              activeTool.kind === "diff" ? " workspace-tool-pane-diff" : ""
            }`}
            role="tabpanel"
            aria-label={activeTool.kind === "diff" ? "Diff" : "File"}
          >
            {activeTool.kind === "diff" ? (
              <ToolDiffPane
                key={activeTool.id}
                workspaceKey={activeTool.workspaceKey}
                path={activeTool.path}
                refreshNonce={toolRefreshNonce}
                cache={toolContentCache.diffs}
              />
            ) : (
              <WorkspaceFileTab
                key={activeTool.id}
                workspaceKey={activeTool.workspaceKey}
                path={activeTool.path}
                refreshNonce={toolRefreshNonce}
                cache={toolContentCache.fileCells}
              />
            )}
          </div>
        ) : paneSession !== null ? (
          <>
            <RecoveredSessionBar
              key={`recovered-bar-${paneSession.id}`}
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
                fileLinks={chatFileLinks}
                observedState={paneSession.state}
                initialGoal={paneSession.goal}
                elapsedMs={paneSession.elapsedMs}
                activity={paneSession.activity}
                attention={activeSessionAttention(paneSession)}
                daemonState={daemon.state}
                sessionRoster={sessions}
                onOpenSubagent={handleOpenSubagent}
                subagentAttention={subagentAttention}
                onRefreshSubagents={refreshSessions}
                headerMenuSeam={{
                  workspaceKey: paneWorkspaceKey,
                  closeEntries: buildTabCloseEntries(
                    composedTabs.findIndex((tab) => tab.id === paneSession.id),
                    composedTabs.length,
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
                pendingPlanToolCallId={pendingPlanToolCallId}
                // The daemon owns this session's follow-up queue (see
                // `queueSupported`): the surface renders its snapshots, and
                // Enter mid-turn queues instead of interrupting.
                queueSupported={queueSupported}
                gifWebpSupported={gifWebpSupported}
                auxiliary={
                  selectedPermission !== null ? (
                    <WorkspacePermissionCard
                      // Session and tool call: a new identity mounts a new
                      // card, so its terminal flag starts from the queue item.
                      key={`${paneSession.id}\n${selectedPermission.request.toolCallId}`}
                      sessionId={paneSession.id}
                      subscriptionId={selectedPermission.subscriptionId}
                      request={selectedPermission.request}
                      capabilities={daemon.capabilities}
                      daemonState={daemon.state}
                      origin={paneSession.origin}
                      deviceNames={peerNames}
                      resolution={selectedPermission.resolution ?? null}
                      creatorId={paneSession.createdBy ?? null}
                      stale={selectedPermission.stale === true}
                      onStale={markPermissionStale}
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
                workspaceKey={selectedKey}
                sessionId={paneSession.id}
                observedState={paneSession.state}
                cwd={paneSession.cwd}
                activity={paneSession.activity}
                attention={activeSessionAttention(paneSession)}
                autoFocus={terminalAutoFocus}
                autoFocusGuard={mayTakeTerminalFocus}
                onAutoFocusTaken={takeTerminalFocus}
                onClosed={handleSessionClosed}
                onExited={handleSessionClosed}
                onCloseTab={() => tabClose.closeSingle(paneSession.id)}
                headerMenuSeam={{
                  workspaceKey: paneWorkspaceKey,
                  closeEntries: buildTabCloseEntries(
                    composedTabs.findIndex((tab) => tab.id === paneSession.id),
                    composedTabs.length,
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
              panelWidth={rightWidth}
            />

            <div
              key={`${selectedSurface.id}:${selectedKey ?? ""}`}
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
                    workspaceKey: selectedKey,
                    // The one fact the Changes panel gates on, computed from
                    // the daemon status this component already holds — a
                    // panel reads it here instead of polling for its own.
                    canListCommits:
                      daemon.state === "connected" &&
                      daemon.capabilities.includes(WORKSPACE_GIT_LOG),
                    // The hand-off the pencils call: a diff tab from
                    // Changes, a file tab from Files (bound in the registry).
                    onOpenFile: openToolTab,
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
