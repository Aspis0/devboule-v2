import {
  dropPermissionRequest,
  syncOpenTabs,
  pendingPermissionRequests,
  subscribePermissions,
} from "../../lib/agentSessionRegistry";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { ErrorText } from "../../components/ErrorText";
import { ConfirmProvider } from "../../components/ConfirmHost";
import { SurfaceErrorBoundary } from "../../app/SurfaceErrorBoundary";
import { NewProjectDialog } from "../../components/NewProjectDialog";
import {
  remoteHostProjectRemove,
  remoteHostWorkspaceArchive,
  remoteHostWorkspaceCreate,
} from "../../lib/tauri";
import type { AgentSession } from "../../lib/agentSession";
import {
  DraftAgentComposer,
  resolveDraftSelection,
  type DraftChoices,
  type DraftSelection,
  type ResolvedDraftSelection,
} from "./DraftAgentComposer";
import { sendFirstMessageWhenReady } from "./firstMessage";
import { NewWorkspaceSheet, THIS_PC, type NewWorkspaceChoice } from "./NewWorkspaceSheet";
import {
  SIDE_PANEL_REGISTRY,
  type AgentTasksContext,
  type SidePanelEntry,
} from "./sidePanelRegistry";
import { childRow } from "./subagentRows";
import { archiveTargets, type ArchiveTarget } from "./subagentArchive";
import { useTasksAttention } from "./useTasksAttention";
import type { BackgroundTaskSource } from "./useBackgroundTaskState";
import { SIDE_PANEL_BODY_ID, SidePanelTabs, sidePanelTabId } from "./panel/SidePanelTabs";
import { PanelStub } from "./PanelToggle";
import { usePanelChords } from "./usePanelChords";
import { useMenuOpen } from "../../lib/menuOpen";
import { TerminalSurface } from "../terminal/TerminalSurface";
import { AgentChatSurface } from "./AgentChatSurface";
import { RemoteWorkspaceSurface } from "./RemoteWorkspaceSurface";
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
import {
  insertionEdge,
  insertionIndex,
  moveTabId,
  orderStripTabs,
  stepTabId,
} from "./strip/tabOrder";
import { readStripOrders, writeStripOrders } from "./strip/stripOrderStorage";
import { readTabSlots } from "./strip/tabSlots";
import { edgeScrollStep } from "./strip/stripEdgeScroll";
import { ToolDiffPane } from "./ToolDiffPane";
import { WorkspaceFileTab } from "./WorkspaceFileTab";
import { BrowserTab } from "./BrowserTab";
import { watchBrowserTabs } from "./browserTabEvents";
import {
  activeBrowserTabFor,
  browserLayoutSnapshot,
  closeBrowserTab,
  openBrowserTab,
  pruneBrowserTabs,
  routeBrowserPopup,
  subscribeBrowserLayout,
} from "./browserTabs";
import { browserTabLabel, normalizeBrowserUrl, BROWSER_START_URL } from "./browserUrl";
import { closeBrowserPage } from "./browserPages";
import { SplitPane } from "./split/SplitPane";
import { SplitDragLayer, type SplitDragLayerHandle } from "./split/SplitDragLayer";
import { resolveDropOutcome, tabCanGoBelow, type DropZone } from "./split/tabDropZones";
import type { TabDropPoint, TabInsertionMark } from "./split/useTabDrag";
import {
  forgetSplitPanesFor,
  mergeSplitPane,
  setSplitPaneSize,
  splitPaneDown,
  splitPaneFor,
  subscribeSplitPanes,
} from "./split/splitPanes";
import { ErrorTriangleIcon } from "./ErrorTriangleIcon";
import { createToolContentCache, evictToolContent } from "./toolContentCache";
import {
  localWorkspaceKey,
  parseWorkspaceKey,
  workspaceKey,
  type WorkspaceKey,
} from "./hosts/hostIdentity";
import { activeTabFor, forgetTab, pruneTabMemory, rememberActiveTab } from "./workspaceTabMemory";
import { useTabSelection } from "./strip/useTabSelection";
import { sessionTabElementId, useTabCloseFlow } from "./strip/useTabCloseFlow";
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
import { MIN_LEFT_WIDTH, MIN_RIGHT_WIDTH, useWorkspacePanelResize } from "./workspaceResize";
import { keyOfWorkspace, useWorkspaceProjects } from "./workspaceProjects";
import { getLastSelectedWorkspaceKey, setLastSelectedWorkspaceKey } from "./lastSelectedWorkspace";
import { Sidebar } from "./sidebar/Sidebar";
import { hostNames } from "./sidebar/hostNames";
import { LOCAL_HOST_ID, type HostId } from "./hosts/hostIdentity";
import type { WorkspaceTreeProps } from "./sidebar/WorkspaceTree";
import { useRailAgents } from "./sidebar/useRailAgents";
import { HistoryPanel } from "../history/HistoryPanel";
import { StatusBar, agentStateOf, type FocusedAgent } from "./statusBar/StatusBar";
import { useWorkspaceStats } from "./sidebar/useWorkspaceStats";
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
  peerDeviceNames,
  sessionCreateFromProvider,
  sessionCreatorTooltip,
  sessionTitle,
  workspacePickerProviders,
  isRecoveredSession,
  sharedSessionController,
  useWorkspaceSessions,
} from "./workspaceSessions";
import {
  heldAssistantTextFor,
  setAttentionHeldContentProvider,
  workspaceHeldContentProvider,
} from "./attentionNotice";
import { setAttentionOpener } from "./attentionActivation";
import { RecoveredSessionBar } from "./recoveredSessionBar";
import { DaemonRestartNotice } from "./daemonRestartNotice";
import type {
  PermissionRequest,
  PermissionResolved,
  ProviderInfo,
  Session,
  SessionKind,
  SessionSearchHit,
} from "../../types/ipc";
import { isAgentKind } from "../../types/ipc";
import {
  daemonRestart,
  providersList,
  sessionClose,
  sessionResume,
  sessionStop,
  workspaceFilesList,
  type AttentionTarget,
  providerVocabularyGet,
  sessionSetModel,
  workspaceFileEditorOpen,
} from "../../lib/tauri";
import { workspaceFileIndex } from "./workspaceFileIndex";
import { openChatFileLink } from "./chatFileOpen";
import type { TranscriptSearchProps } from "./sidebar/TranscriptSearchResults";
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
/** The negotiated capability the composer's chunked file upload is gated on. */
const ATTACHMENTS_UPLOAD_CAPABILITY = "attachments.upload";
const SESSIONS_SEARCH_CAPABILITY = "sessions.search";
/** Said wherever a tab or agent is asked for with no workspace selected. */
const NO_WORKSPACE_SENTENCE = "No workspace is selected.";
const NO_WORKSPACE_ERROR: ErrorSentence = { sentence: NO_WORKSPACE_SENTENCE, detail: null };
/** The empty pane's reason, linked to its disabled action. */
const EMPTY_PANE_REASON_ID = "workspace-empty-reason";

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

/**
 * A new workspace's first prompt, held until its agent confirms it. `queued`
 * waits for the agent to attach, `sending` is the send in flight, `failed`
 * means the agent refused it, and `handed` means the pane closed before the
 * send: in both of the last two the text waits for the person to send it.
 */
interface FirstPrompt {
  workspaceKey: WorkspaceKey;
  sessionId: string;
  text: string;
  state: "queued" | "sending" | "failed" | "handed";
  /** Bumped when the text is handed back, so the composer restores it once. */
  attempt: number;
  /** The model or effort did not take: the next send applies them again first. */
  settingsPending?: boolean;
}

const FIRST_PROMPT_FAILED =
  "The agent did not take the message. It is still here: press send to try again.";
const FIRST_PROMPT_HANDED =
  "Not sent: the agent closed before it took the message. It is still here: press send to send it.";
const FIRST_PROMPT_SETTINGS =
  "The agent started, but its model or effort could not be set. It is still here: press send to set them again and send it.";
const NO_DRAFT_SELECTION: DraftSelection = { modelId: null, effort: null, modeId: null };
/** The model and effort the composer shows, set on the session. A mode is not set
 * here: it rides the create. */
function applyDraftModel(sessionId: string, picks: ResolvedDraftSelection): Promise<void> {
  if (picks.modelId === null) return Promise.resolve();
  return sessionSetModel(sessionId, picks.modelId, picks.effort ?? undefined);
}
const FIRST_PROMPT_NOT_ATTACHED =
  "The agent is not attached right now. Open the workspace's agent, then press send.";

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
    archiveWorkspace,
    deleteWorktree,
    removeProject,
    createWorkspace,
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

  // The selected workspace's host, when it is not this machine: the surface
  // switches to the read-only remote view for it, and the local strip and
  // chat panes keep their state for when a local workspace is selected again.
  const selectedRemoteHost = useMemo(() => {
    if (selectedKey === null) return null;
    const parsed = parseWorkspaceKey(selectedKey);
    return parsed.hostId === LOCAL_HOST_ID ? null : parsed;
  }, [selectedKey]);
  const {
    leftWidth,
    rightWidth,
    leftMax,
    rightMax,
    leftCollapsed,
    rightCollapsed,
    setLeftCollapsed,
    setRightCollapsed,
    startDrag,
    handleResizeKey,
  } = useWorkspacePanelResize();
  usePanelChords({ leftCollapsed, rightCollapsed, setLeftCollapsed, setRightCollapsed });
  const [activeSidePanel, setActiveSidePanel] = useState<ActiveSidePanel>("changes");
  // The front pane's session controller, as its surface reported it. The Tasks
  // tab reads its task lane only while the pane still shows that session.
  const [paneAgent, setPaneAgent] = useState<{
    sessionId: string;
    source: BackgroundTaskSource;
  } | null>(null);
  const tasksVisible = activeSidePanel === "tasks" && !rightCollapsed;
  const tasksUnseen = useTasksAttention(paneAgent, tasksVisible);
  const [historyOpen, setHistoryOpen] = useState(false);
  // The message the sidebar's search opened a session at: the transcript scrolls
  // to it once. The nonce makes a second open of the same message scroll again.
  const [transcriptFocus, setTranscriptFocus] = useState<{
    sessionId: string;
    messageId: string | null;
    nonce: number;
  } | null>(null);
  const [historySearch, setHistorySearch] = useState("");
  // The workspace the History page lists when opened from a rail's overflow; null lists all.
  const [historyWorkspace, setHistoryWorkspace] = useState<WorkspaceKey | null>(null);
  const [historyHost, setHistoryHost] = useState("all");
  const [permissionQueue, setPermissionQueue] =
    useState<QueuedPermission[]>(pendingPermissionRequests);
  // Drop one session's cards. The close acts call this on success and on a
  // moot close, and the roster absence rule below calls it when the row is
  // gone: a failed close while the session still exists keeps them.
  const dropSessionPermissions = useCallback((id: string) => {
    setPermissionQueue((queue) => queue.filter((item) => item.sessionId !== id));
  }, []);
  // What a toast may quote for a session — the pending permission card's
  // text and the last assistant message — is wired below, once the strip's
  // own rows exist: the provider's inputs are what this render puts on
  // screen, never a ref a later effect fills.
  const daemon = useWorkspaceDaemon();
  const devices = usePairedDevices();
  // Each host's short label by id, local first: the sidebar rows' second
  // line and the History page's host filter read the same map.
  const hostNameMap = useMemo(() => hostNames(devices), [devices]);
  // The local machine answers by construction; a peer's state is the union
  // the daemon reports (inbound or outbound link), so both Devices panels
  // read online for the same connection.
  const hostOnlineMap = useMemo(() => {
    const online = new Map<HostId, boolean>([[LOCAL_HOST_ID, true]]);
    for (const peer of devices.peers) online.set(peer.deviceId as HostId, peer.online);
    return online;
  }, [devices]);
  const historyHosts = useMemo(
    () => [...hostNameMap].map(([id, name]) => ({ id, name })),
    [hostNameMap],
  );
  // Device id to display name, for the tab badge that names a peer session's
  // device. The sidebar's host list reads the same payload on the same poll, so
  // the names are derived from that read rather than from a second one.
  const peerNames = useMemo(() => peerDeviceNames(devices.peers), [devices]);
  const queueSupported = daemon.capabilities.includes(SESSION_QUEUE_CAPABILITY);
  const gifWebpSupported = daemon.capabilities.includes(ATTACHMENTS_GIF_WEBP_CAPABILITY);
  const fileUploadSupported = daemon.capabilities.includes(ATTACHMENTS_UPLOAD_CAPABILITY);
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
  useEffect(() => {
    syncOpenTabs(openSessions.map((session) => session.id));
  }, [openSessions]);
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
  // The order the person arranged each workspace's strip in, every kind of tab
  // in one list. Written through on every drop, so it outlives the mount.
  const [stripOrders, setStripOrders] = useState(readStripOrders);
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
  const handleArchiveWorkspace = useCallback(
    async (workspaceId: string, hostId: string = LOCAL_HOST_ID): Promise<ErrorSentence | null> => {
      if (hostId !== LOCAL_HOST_ID) {
        try {
          await remoteHostWorkspaceArchive(hostId, workspaceId);
          return null;
        } catch (cause: unknown) {
          return errorSentence(cause);
        }
      }
      const refusal = await archiveWorkspace(workspaceId);
      // The daemon's row is off the list; History can keep naming the id for
      // its sessions, so the cache is dropped here and never waits for History.
      const key = localWorkspaceKey(workspaceId);
      if (refusal === null && key !== null) evictWorkspaceStats(key);
      return refusal;
    },
    [archiveWorkspace, evictWorkspaceStats],
  );
  const handleDeleteWorktree = useCallback(
    async (workspaceId: string): Promise<ErrorSentence | null> => {
      const refusal = await deleteWorktree(workspaceId);
      // The folder is gone with the row, so its cached stats go with it.
      const key = localWorkspaceKey(workspaceId);
      if (refusal === null && key !== null) evictWorkspaceStats(key);
      return refusal;
    },
    [deleteWorktree, evictWorkspaceStats],
  );
  const handleRemoveProject = useCallback(
    async (projectId: string, hostId: string = LOCAL_HOST_ID): Promise<ErrorSentence | null> => {
      if (hostId !== LOCAL_HOST_ID) {
        try {
          await remoteHostProjectRemove(hostId, projectId);
          return null;
        } catch (cause: unknown) {
          return errorSentence(cause);
        }
      }
      const refusal = await removeProject(projectId);
      // Every row the project listed is off the list with it.
      if (refusal === null) {
        for (const project of projects) {
          if (project.id !== projectId) continue;
          for (const workspace of project.workspaces) {
            const key = keyOfWorkspace(workspace);
            if (key !== null) evictWorkspaceStats(key);
          }
        }
      }
      return refusal;
    },
    [evictWorkspaceStats, projects, removeProject],
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
    forgetSplitPanesFor(knownWorkspaceKeys);
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
  /** The workspace's split: one tool tab in a pane below the pane the centre
   * has always shown, and how tall that pane is. Scoped by workspace key, so
   * switching workspaces never carries a split along. */
  const split = useSyncExternalStore(subscribeSplitPanes, () =>
    selectedKey === null ? null : splitPaneFor(selectedKey),
  );
  const lowerTabId = split?.lowerTabId ?? null;
  /** The tab in the pane below, resolved against the tabs this workspace
   * really has. A record naming a tab that has gone is not a split. */
  const lowerTool = visibleToolTabs.find((tab) => tab.id === lowerTabId) ?? null;
  const lowerBrowserId = lowerTool?.kind === "browser" ? lowerTool.browserId : null;
  useEffect(() => {
    // The pane below is empty: the workspace is one pane again, and the record
    // of a tab that no longer exists is dropped with it.
    if (selectedKey !== null && split !== null && lowerTool === null) mergeSplitPane(selectedKey);
  }, [lowerTool, selectedKey, split]);
  /**
   * What each pane holds, in one answer. Every reader of "is a tool tab or a
   * conversation on screen" asks this and not `activeTool`: a page cannot be
   * drawn in two panes at once, so the tab in the pane below stops filling the
   * pane above, and a reader keyed on the strip's selection alone would go on
   * answering as if that pane were still covered.
   */
  const panes = useMemo(() => {
    const upperTool = lowerTabId !== null && activeTool?.id === lowerTabId ? null : activeTool;
    return {
      lowerTool,
      lowerBrowserId,
      upperTool,
      upperToolId: upperTool?.id ?? null,
      upperSession: upperTool === null ? paneSessionOf(selectedSessionId, visibleSessions) : null,
    };
  }, [activeTool, lowerBrowserId, lowerTabId, lowerTool, selectedSessionId, visibleSessions]);
  /** What the pane below is showing, named in its header: the same words the
   * strip's chip names the tab by. */
  const lowerRecord =
    lowerBrowserId === null
      ? null
      : (browserLayout.tabs.find((tab) => tab.browserId === lowerBrowserId) ?? null);
  const lowerLabel =
    lowerRecord === null ? "" : browserTabLabel(lowerRecord.title, lowerRecord.url);
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
    () =>
      orderStripTabs(
        composeStripTabs(visibleSessions, visibleToolTabs),
        (selectedKey === null ? undefined : stripOrders.get(selectedKey)) ?? [],
        // A recovered session names the one it replaces as its context.
        (tab) => (tab.type === "session" ? (tab.session.contextId ?? null) : null),
      ),
    [visibleSessions, visibleToolTabs, selectedKey, stripOrders],
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
  // A tab an agent opened arrives from Rust as a chip for that workspace, and
  // an agent's `close_tab` takes it away again.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let live = true;
    void watchBrowserTabs()
      .then((stop) => {
        if (live) unlisten = stop;
        else stop();
      })
      // The strip works without this; a tab an agent opened simply gets no
      // chip until the app is restarted.
      .catch((error: unknown) => {
        console.warn("Could not follow browser tab events.", error);
      });
    return () => {
      live = false;
      unlisten?.();
    };
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
  const transcriptSearchCanSearch = daemon.capabilities.includes(SESSIONS_SEARCH_CAPABILITY);
  const openTranscriptHit = useCallback(
    (hit: SessionSearchHit) => {
      const session = sessions.find((row) => row.id === hit.sessionId);
      if (session === undefined) return;
      standDownToolTab();
      openSession(session);
      setTranscriptFocus((previous) => ({
        sessionId: hit.sessionId,
        messageId: hit.messageId,
        nonce: (previous?.nonce ?? 0) + 1,
      }));
    },
    [openSession, sessions, standDownToolTab],
  );
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
  const resizeSplit = useCallback(
    (size: number) => {
      if (selectedKey !== null) setSplitPaneSize(selectedKey, size);
    },
    [selectedKey],
  );
  /** The control that merged the panes is inside the subtree that merge
   * removes, so focus has to land somewhere on purpose: on the tab that was
   * below, whose chip is where the eye already is. Applied after the commit
   * that removed the split, because the chip only exists then. */
  const [movedTab, setMovedTab] = useState<{ tabId: string; sentence: string } | null>(null);
  const [paneAnnouncement, setPaneAnnouncement] = useState("");
  useEffect(() => {
    if (movedTab === null) return;
    // The control that started the move is gone with the pane, so focus lands on
    // the chip of the tab that moved. A keyboard that never saw the pane change
    // hears the sentence instead, and it stays in the region: a status region
    // that empties itself in the same commit announces nothing.
    document.getElementById(sessionTabElementId(movedTab.tabId))?.focus({ preventScroll: true });
    setPaneAnnouncement(movedTab.sentence);
    setMovedTab(null);
  }, [movedTab]);
  /** Whether a tab id is still one of this strip's: an agent closing the dragged
   * tab ends the gesture rather than letting a drop name a tab that is gone. */
  const hasLiveTab = useCallback(
    (tabId: string) => composedTabs.some((tab) => tab.id === tabId),
    [composedTabs],
  );
  const mergeSplit = useCallback(() => {
    if (selectedKey === null) return;
    const merged = splitPaneFor(selectedKey)?.lowerTabId ?? null;
    mergeSplitPane(selectedKey);
    // Merged into the strip means merged into the pane: the tab comes back as
    // the one the workspace is showing, so its page is presented again.
    if (merged === null) return;
    writeToolTab(merged);
    setMovedTab({ tabId: merged, sentence: "Moved out of the pane below." });
  }, [selectedKey, writeToolTab]);
  /** Move the tab in front into the pane below, and put a session on top of
   * it: the pane above is the workspace's own pane, and an empty one is not a
   * split. This is the keyboard's road into the split (the drag is slice 2). */
  const splitTabDown = useCallback(
    (tab: ToolTab) => {
      if (selectedKey === null) return;
      splitPaneDown(selectedKey, tab.id);
      selectSession(selectedSessionId ?? visibleSessions[0]?.id ?? null);
      setMovedTab({ tabId: tab.id, sentence: "Moved to the pane below." });
    },
    [selectSession, selectedKey, selectedSessionId, visibleSessions],
  );
  /** The menu's version of the same act, on a tab id: the strip's menu names a
   * tab, and only the tab model knows what kind it is. */
  const splitDownFromMenu = useCallback(
    (tabId: string) => {
      const tool = visibleToolTabs.find((tab) => tab.id === tabId);
      if (tool === undefined) return;
      splitTabDown(tool);
    },
    [splitTabDown, visibleToolTabs],
  );

  /** A dropped tab, and what its landing does. The panes are read here, at the
   * drop, so the outcome is never decided against a layout the drag has since
   * changed. */
  const dropTab = useCallback(
    (tabId: string, zone: DropZone | "strip", point: TabDropPoint) => {
      if (selectedKey === null) return;
      const outcome = resolveDropOutcome({
        zone,
        draggedTabId: tabId,
        lowerTabId: split?.lowerTabId ?? null,
        upperTabId: activeTabId,
        upperCanMoveBelow: panes.upperTool?.kind === "browser",
      });
      if (outcome.kind === "split-down") {
        splitPaneDown(selectedKey, tabId);
        selectSession(selectedSessionId ?? visibleSessions[0]?.id ?? null);
        return;
      }
      if (outcome.kind === "split-up") {
        // The tab that was above moves below and the dragged one takes its place,
        // so what the person pointed at is what they get. Both are browser tabs:
        // the rule refuses every other pair, and the pane renders nothing else.
        const moved = panes.upperTool?.id;
        if (moved === undefined || !tabCanGoBelow(tabId)) return;
        splitPaneDown(selectedKey, moved);
        writeToolTab(tabId);
        setMovedTab({ tabId, sentence: "Moved to the pane above." });
        return;
      }
      if (outcome.kind === "merge") {
        mergeSplitPane(selectedKey);
        writeToolTab(tabId);
        setMovedTab({ tabId, sentence: "Moved out of the pane below." });
        return;
      }
      if (outcome.kind === "reorder") {
        const strip = document.querySelector<HTMLElement>(".workspace-session-tabs");
        if (strip === null) return;
        const index = insertionIndex(readTabSlots(strip), point.x, tabId);
        const next = moveTabId(
          composedTabs.map((tab) => tab.id),
          tabId,
          index,
        );
        const orders = new Map(stripOrders).set(selectedKey, next);
        setStripOrders(orders);
        writeStripOrders(orders);
      }
      selectTab(tabId);
    },
    [
      activeTabId,
      composedTabs,
      panes.upperTool,
      selectSession,
      selectTab,
      selectedKey,
      stripOrders,
      selectedSessionId,
      split,
      visibleSessions,
      writeToolTab,
    ],
  );
  /** The gap a tab dropped on the row would land in, read from the live chips:
   * the same index the drop itself resolves, so the line shows where it lands. */
  const tabInsertionAt = useCallback(
    (tabId: string, point: TabDropPoint): TabInsertionMark | null => {
      const strip = document.querySelector<HTMLElement>(".workspace-session-tabs");
      if (strip === null) return null;
      const slots = readTabSlots(strip);
      const x = insertionEdge(slots, tabId, insertionIndex(slots, point.x, tabId));
      if (x === null) return null;
      const box = strip.getBoundingClientRect();
      return { x, top: box.top, height: box.height };
    },
    [],
  );
  /** The drag belongs to the layer below, which also draws the preview: a
   * pointer moving inside one destination must not re-render this surface. */
  /** Moves a tab one place along the row, from the keyboard; the order is kept like a drop keeps it. */
  const moveStripTab = useCallback(
    (tabId: string, step: number) => {
      if (selectedKey === null) return;
      const next = stepTabId(
        composedTabs.map((tab) => tab.id),
        tabId,
        step,
      );
      const orders = new Map(stripOrders).set(selectedKey, next);
      setStripOrders(orders);
      writeStripOrders(orders);
    },
    [composedTabs, selectedKey, stripOrders],
  );
  /** Scrolls the tab row while the pointer holds one of its ends, and says so. */
  const scrollStripAt = useCallback((x: number): boolean => {
    const scroll = document.querySelector<HTMLElement>(".workspace-session-tabs-scroll");
    if (scroll === null) return false;
    const box = scroll.getBoundingClientRect();
    const step = edgeScrollStep(x, box.left, box.right);
    if (step === 0) return false;
    const before = scroll.scrollLeft;
    scroll.scrollLeft += step;
    return scroll.scrollLeft !== before;
  }, []);
  const dragLayerRef = useRef<SplitDragLayerHandle>(null);
  const startTabDrag = useCallback(
    (tabId: string, owner: Element, event: ReactPointerEvent<HTMLDivElement>) => {
      dragLayerRef.current?.start(tabId, owner, event);
    },
    [],
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
    (
      kind: SessionKind,
      provider: string | null,
      workspaceId: string | null,
      mode: string | null = null,
    ) => {
      const toolTabAtStart = activeToolTabIdRef.current;
      const toolNavAtStart = toolNavRef.current;
      return createSession(kind, provider, workspaceId, mode).then((session) => {
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
        closeBrowserTab(record.browserId);
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
    lowerTabId: split?.lowerTabId ?? null,
    onSplitDown: splitDownFromMenu,
    // The menu's way out of the pane below is the merge control's act: the tab
    // comes back up and the workspace is one pane again.
    onMoveUpPane: mergeSplit,
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
  // The centre shows the tool tab while one is in front; the session pane
  // only stands when no tool covers it — its queue, permission card and
  // header menu unmount with it, so none of them act on the hidden session.
  // Both come out of `panes`, so every reader of "what is on screen" agrees
  // with every other one, including while a split is open.
  const paneTool = panes.upperTool;
  const paneSession = panes.upperSession;
  // The agent the status bar speaks for: the one in the front pane, never a
  // session hidden behind a tool tab.
  const focusedAgent: FocusedAgent | null =
    paneTool === null && paneSession !== null && isAgentKind(paneSession.kind)
      ? {
          sessionId: paneSession.id,
          title: sessionTitle(paneSession),
          ...agentStateOf(paneSession),
        }
      : null;
  const statusProgress = sessionCreating
    ? "Starting…"
    : sessionsLoading && sessions.length === 0
      ? "Loading…"
      : null;
  // Primitive dependencies keep fileLinks stable across roster pushes so
  // unchanged messages retain their Markdown memo.
  // The pane's workspace, as the UI names it: the header menu reads it for the
  // branch row, the file links open tabs under it.
  const paneWorkspaceId = paneSession?.workspaceId ?? null;
  const paneWorkspaceKey = paneWorkspaceId === null ? null : localWorkspaceKey(paneWorkspaceId);
  const paneCwd = paneSession?.cwd;
  const chatFileLinks = useMemo(() => {
    if (paneWorkspaceId === null || paneCwd === undefined || paneWorkspaceKey === null) return null;
    const workspaceId = paneWorkspaceId;
    return {
      root: paneCwd,
      open: (relativePath: string) =>
        openChatFileLink(
          relativePath,
          (path) => workspaceFileEditorOpen(workspaceId, path),
          (path) => openToolTab(paneWorkspaceKey, path, "file"),
        ),
    };
  }, [paneWorkspaceId, paneCwd, paneWorkspaceKey, openToolTab]);
  // The "@" menu's file index: built on the first mention, per workspace.
  const loadPaneFileIndex = useMemo(() => {
    if (paneWorkspaceId === null) return undefined;
    const workspaceId = paneWorkspaceId;
    return () => workspaceFileIndex(workspaceId, (path) => workspaceFilesList(workspaceId, path));
  }, [paneWorkspaceId]);
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
  /** The centre a drop is read against, read live: the window resizes it. */
  const centerRef = useRef<HTMLElement>(null);

  const upperToolIdRef = useRef(panes.upperToolId);
  useEffect(() => {
    upperToolIdRef.current = panes.upperToolId;
  });
  useLayoutEffect(() => {
    const controller = sharedSessionController();
    const current = (): string | null =>
      upperToolIdRef.current !== null ? null : controller.getState().selectedSessionId;
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
      panes.upperToolId === null ? sharedSessionController().getState().selectedSessionId : null,
    );
  }, [panes.upperToolId]);
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
  // The Tasks tab's open road: the who-is-working list's road, except that a child the
  // roster does not list says so rather than doing nothing.
  const openTaskAgent = useCallback(
    (sessionId: string): string | null => {
      const session = sessions.find((row) => row.id === sessionId);
      if (session === undefined) {
        void refreshSessions();
        return "This agent was archived.";
      }
      handleReopenSession(session);
      return null;
    },
    [sessions, handleReopenSession, refreshSessions],
  );
  const childInfoOf = useCallback(
    (sessionId: string) => {
      const session = sessions.find((row) => row.id === sessionId);
      return session === undefined
        ? undefined
        : {
            kind: session.kind,
            title: sessionTitle(session),
            provider: session.provider ?? session.kind,
          };
    },
    [sessions],
  );
  const reportPaneAgent = useCallback(
    (sessionId: string, agent: BackgroundTaskSource | null) =>
      setPaneAgent(agent === null ? null : { sessionId, source: agent }),
    [],
  );
  // Stop keeps the child's session and transcript (the strip's stop road). The
  // roster is read first, so a child that settled meanwhile is not stopped.
  const stopAgent = useCallback(
    async (childSessionId: string): Promise<string | null> => {
      const row = sessions.find((session) => session.id === childSessionId);
      if (row === undefined || childRow(row).status !== "running") {
        return "It is no longer running, so nothing was stopped.";
      }
      try {
        await sessionStop(childSessionId);
        return null;
      } catch (cause) {
        return errorSentence(cause).sentence;
      }
    },
    [sessions],
  );
  const agentTasks = useMemo<AgentTasksContext | null>(
    () =>
      paneSession !== null && paneAgent?.sessionId === paneSession.id
        ? {
            sessionId: paneSession.id,
            source: paneAgent.source,
            childInfo: childInfoOf,
            onOpenAgent: openTaskAgent,
            onStopAgent: stopAgent,
          }
        : null,
    [paneSession, paneAgent, childInfoOf, openTaskAgent, stopAgent],
  );
  // A failed resume leaves the row's verdict changed on the daemon side; the
  // bar must not keep its offer on the roster data this surface already held.
  const handleResumeFailed = useCallback(() => {
    void refreshSessions();
  }, [refreshSessions]);
  // A stopped session's composer resumes it: the same reopen road the recovered
  // bar takes, so the pane opens the live session. The text stays in the composer.
  const resumeStoppedSession = useCallback(
    async (session: Session): Promise<boolean> => {
      const result = await sessionResume(session.id).catch(() => null);
      if (result?.type === "resumed") {
        handleReopenSession(result.session);
        return true;
      }
      handleResumeFailed();
      return false;
    },
    [handleReopenSession, handleResumeFailed],
  );
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
    return workspacePickerProviders(catalog.providers);
  }, []);
  const startAgentSession = useCallback(
    (provider: ProviderInfo | undefined, workspaceId: string | null) => {
      const args = sessionCreateFromProvider(provider);
      void createAndShowSession(args.kind, args.provider, workspaceId);
    },
    [createAndShowSession],
  );
  const addSessionToWorkspace = useCallback(
    (provider: ProviderInfo | undefined) => {
      startAgentSession(provider, selectedWorkspaceId);
      endProviderChoice();
    },
    [endProviderChoice, selectedWorkspaceId, startAgentSession],
  );
  /**
   * One provider-choice flow for every button that starts an agent: 0 capable
   * providers open the empty picker, one falls straight through, two or more
   * open the picker. What happens after the choice is the caller's `afterChoice`.
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
      if (capable.length === 1) {
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
      setProviderPicker(capable);
    },
    [endProviderChoice, loadChatProviders, selectedKey],
  );
  /** The workspace a sheet just created: its pane shows the first-message composer until the prompt is confirmed. */
  const [draftKey, setDraftKey] = useState<WorkspaceKey | null>(null);
  /** The chat-capable providers for the draft composer; null while they load. */
  const [draftProviders, setDraftProviders] = useState<ProviderInfo[] | null>(null);
  const [draftProviderId, setDraftProviderId] = useState<string | null>(null);
  /** The chosen provider's models and modes, as its catalog reports them. */
  const [draftChoices, setDraftChoices] = useState<DraftChoices | null>(null);
  const [draftSelection, setDraftSelection] = useState<DraftSelection>(NO_DRAFT_SELECTION);
  const [firstPrompt, setFirstPromptState] = useState<FirstPrompt | null>(null);
  /** The ref mirrors the state for the callbacks, which must see the latest prompt. */
  const firstPromptRef = useRef<FirstPrompt | null>(null);
  // A first send from the press to the agent's answer. The ref is the guard; the
  // state shows it in the composer.
  const draftStartingRef = useRef(false);
  const [draftStarting, setDraftStarting] = useState(false);
  const setFirstPrompt = useCallback((next: FirstPrompt | null) => {
    firstPromptRef.current = next;
    setFirstPromptState(next);
  }, []);
  const [firstPromptNotice, setFirstPromptNotice] = useState<string | null>(null);
  /** The composer's pending answer: resolved once the agent confirms, refuses, or the pane closes. */
  const firstPromptAnswerRef = useRef<((sent: boolean) => void) | null>(null);
  const firstPromptCancelRef = useRef<(() => void) | null>(null);
  /** The agent of the session the pane shows, for a retry from the composer. */
  const paneAgentRef = useRef<{ sessionId: string; agent: AgentSession } | null>(null);
  // The new-workspace sheet is mounted per opening, keyed by its nonce, so each
  // one starts from the project it was asked for and the defaults.
  const [sheet, setSheet] = useState<{ nonce: number; projectId: string | null } | null>(null);
  const sheetTriggerRef = useRef<HTMLButtonElement | null>(null);
  const handleNewWorkspace = useCallback((trigger: HTMLButtonElement, projectId: string) => {
    sheetTriggerRef.current = trigger;
    setSheet((open) => ({ nonce: (open?.nonce ?? 0) + 1, projectId }));
  }, []);
  const closeSheet = useCallback(() => {
    setSheet(null);
    sheetTriggerRef.current?.focus({ preventScroll: true });
  }, []);
  useEffect(() => {
    if (draftKey === null) return undefined;
    let current = true;
    void loadChatProviders().then(
      (providers) => {
        if (current) setDraftProviders(providers);
      },
      (cause: unknown) => {
        if (current) {
          setDraftProviders([]);
          setProviderError(errorSentence(cause));
        }
      },
    );
    return () => {
      current = false;
    };
  }, [draftKey, loadChatProviders]);
  const draftProviderPick =
    (
      (draftProviders ?? []).find((candidate) => candidate.id === draftProviderId) ??
      draftProviders?.[0]
    )?.id ?? null;
  // The chosen provider's catalog, read before any agent runs: its models, their
  // efforts, and its modes.
  useEffect(() => {
    setDraftChoices(null);
    if (draftProviderPick === null) return undefined;
    let current = true;
    void providerVocabularyGet(draftProviderPick, "", false).then(
      (vocabulary) => {
        if (current) {
          setDraftChoices({ models: vocabulary.models.items, modes: vocabulary.modes.items });
        }
      },
      () => {
        if (current) setDraftChoices(null);
      },
    );
    return () => {
      current = false;
    };
  }, [draftProviderPick]);
  const pickDraftProvider = useCallback((providerId: string) => {
    setDraftProviderId(providerId);
    setDraftSelection(NO_DRAFT_SELECTION);
  }, []);
  // The composer's answer: a confirmed prompt clears the draft, a refused one
  // stays in the composer for the person to send again.
  const answerFirstPrompt = useCallback(
    (sent: boolean) => {
      const answer = firstPromptAnswerRef.current;
      firstPromptAnswerRef.current = null;
      firstPromptCancelRef.current = null;
      const current = firstPromptRef.current;
      if (current !== null) {
        if (sent) {
          setFirstPrompt(null);
          setDraftKey(null);
          setFirstPromptNotice(null);
        } else {
          setFirstPrompt({ ...current, state: "failed" });
          setFirstPromptNotice(FIRST_PROMPT_FAILED);
        }
      }
      answer?.(sent);
    },
    [setFirstPrompt],
  );
  // Sends the held prompt once the agent is attached and idle; the answer goes to
  // the composer's pending promise. Nothing is sent again by this wait.
  const startFirstPromptSend = useCallback(
    (sessionId: string, agent: AgentSession, text: string) => {
      firstPromptCancelRef.current?.();
      const current = firstPromptRef.current;
      if (current !== null) setFirstPrompt({ ...current, sessionId, text, state: "sending" });
      firstPromptCancelRef.current = sendFirstMessageWhenReady(agent, text, answerFirstPrompt);
    },
    [answerFirstPrompt, setFirstPrompt],
  );
  // Starts the agent the first message is for, with the provider picked in the
  // composer. The composer keeps its text until the agent confirms it.
  const startDraft = useCallback(
    async (text: string): Promise<boolean> => {
      if (draftKey === null) return false;
      const current = firstPromptRef.current;
      if (current !== null && current.workspaceKey === draftKey) {
        // A retry: the prompt goes to the agent it was held for, if that agent is open.
        const attached = paneAgentRef.current;
        if (attached === null || attached.sessionId !== current.sessionId) {
          setFirstPromptNotice(FIRST_PROMPT_NOT_ATTACHED);
          return false;
        }
        // A model or effort that did not take the first time is set again, from
        // what the composer shows now, before the prompt goes.
        if (current.settingsPending === true) {
          try {
            await applyDraftModel(
              current.sessionId,
              resolveDraftSelection(draftChoices, draftSelection),
            );
          } catch {
            setFirstPrompt({ ...current, state: "failed" });
            setFirstPromptNotice(FIRST_PROMPT_SETTINGS);
            return false;
          }
          setFirstPrompt({ ...current, settingsPending: false });
        }
        return new Promise<boolean>((answer) => {
          firstPromptAnswerRef.current = answer;
          setFirstPromptNotice(null);
          startFirstPromptSend(current.sessionId, attached.agent, text);
        });
      }
      const providers = draftProviders ?? [];
      const provider =
        providers.find((candidate) => candidate.id === draftProviderId) ?? providers[0];
      // With no provider to start there is no agent to send to: the text stays.
      if (provider === undefined) return false;
      const args = sessionCreateFromProvider(provider);
      // The mode goes with the create, so the agent starts in it and no prompt
      // can go out before it is set. The model and effort follow the create.
      const picks = resolveDraftSelection(draftChoices, draftSelection);
      const session = await createAndShowSession(
        args.kind,
        args.provider,
        parseWorkspaceKey(draftKey).workspaceId,
        picks.modeId,
      );
      if (session === null) {
        setFirstPromptNotice("The agent did not start. Your message is still here.");
        return false;
      }
      // A model or effort that does not take holds the prompt for the person:
      // nothing is sent with other settings, and the next send sets them again.
      try {
        await applyDraftModel(session.id, picks);
      } catch {
        setFirstPrompt({
          workspaceKey: draftKey,
          sessionId: session.id,
          text,
          state: "failed",
          attempt: 0,
          settingsPending: true,
        });
        setFirstPromptNotice(FIRST_PROMPT_SETTINGS);
        return false;
      }
      setFirstPrompt({
        workspaceKey: draftKey,
        sessionId: session.id,
        text,
        state: "queued",
        attempt: 0,
      });
      setFirstPromptNotice(null);
      return new Promise<boolean>((answer) => {
        firstPromptAnswerRef.current = answer;
      });
    },
    [
      createAndShowSession,
      draftChoices,
      draftKey,
      draftProviderId,
      draftProviders,
      draftSelection,
      setFirstPrompt,
      startFirstPromptSend,
    ],
  );
  // One first send at a time. The start is awaited in several steps (the agent,
  // its settings, the prompt's answer), and the composer keeps its text until the
  // answer, so a second press in that window must not start a second agent: the
  // ref refuses it at once, and the state disables the composer while it shows.
  const sendDraft = useCallback(
    async (text: string): Promise<boolean> => {
      if (draftStartingRef.current) return false;
      draftStartingRef.current = true;
      setDraftStarting(true);
      try {
        return await startDraft(text);
      } finally {
        draftStartingRef.current = false;
        setDraftStarting(false);
      }
    },
    [startDraft],
  );
  // The pane reports its agent as it attaches and as it leaves. A held prompt is
  // sent once, when its own agent first attaches; a pane that closes first hands
  // the text back instead.
  const reportPaneAgentWithFirstMessage = useCallback(
    (sessionId: string, agent: AgentSession | null) => {
      reportPaneAgent(sessionId, agent);
      if (agent === null) {
        if (paneAgentRef.current?.sessionId === sessionId) paneAgentRef.current = null;
        const current = firstPromptRef.current;
        if (
          current !== null &&
          current.sessionId === sessionId &&
          (current.state === "queued" || current.state === "sending")
        ) {
          firstPromptCancelRef.current?.();
          firstPromptCancelRef.current = null;
          setFirstPrompt({ ...current, state: "handed", attempt: current.attempt + 1 });
          setFirstPromptNotice(FIRST_PROMPT_HANDED);
          const answer = firstPromptAnswerRef.current;
          firstPromptAnswerRef.current = null;
          answer?.(false);
        }
        return;
      }
      paneAgentRef.current = { sessionId, agent };
      const current = firstPromptRef.current;
      if (current !== null && current.sessionId === sessionId && current.state === "queued") {
        startFirstPromptSend(sessionId, agent, current.text);
      }
    },
    [reportPaneAgent, setFirstPrompt, startFirstPromptSend],
  );
  // The sheet creates the workspace and selects it; the workspace then waits
  // for its first message, with the provider chosen in its composer.
  // The sheet names this machine THIS_PC; a paired host's projects keep its id.
  const sheetProjects = useMemo(
    () =>
      projects.map((project) => ({
        id: project.id,
        name: project.name,
        hostId: project.hostId === LOCAL_HOST_ID ? THIS_PC : project.hostId,
      })),
    [projects],
  );
  const pairedHostsForSheet = useMemo(
    () =>
      [...hostNameMap]
        .filter(([hostId]) => hostId !== LOCAL_HOST_ID)
        .map(([hostId, name]) => ({
          id: hostId,
          name,
          online: hostOnlineMap.get(hostId) ?? false,
        })),
    [hostNameMap, hostOnlineMap],
  );
  // The workspace a sheet just made shows its first-message composer, even when the
  // pane still remembers a tab of the workspace it came from.
  const draftShowing = draftKey !== null && draftKey === selectedKey;
  const createFromSheet = useCallback(
    async (choice: NewWorkspaceChoice): Promise<ErrorSentence | null> => {
      if (choice.where !== THIS_PC) {
        // A paired host creates the workspace with its own checks. The new
        // workspace opens on that host's surface, where its agent starts through
        // the relay under the person's own grants: nothing is left to find. Its
        // row joins the host's list when the host next publishes it.
        const outcome = await remoteHostWorkspaceCreate(
          choice.where,
          choice.projectId,
          choice.isolation,
          choice.branch,
        ).then(
          (created) => ({ key: workspaceKey(choice.where as HostId, created.id) }),
          (cause: unknown) => ({ refusal: errorSentence(cause) }),
        );
        if ("refusal" in outcome) return outcome.refusal;
        if (outcome.key !== null) {
          userNavigatedRef.current = true;
          setSelectedKey(outcome.key);
        }
        setSheet(null);
        sheetTriggerRef.current?.focus({ preventScroll: true });
        return null;
      }
      const { key, refusal } = await createWorkspace(
        choice.projectId,
        choice.isolation,
        choice.branch,
      );
      if (refusal !== null) return refusal;
      // The person created this workspace: the view stays where they put it,
      // as it does after a row click, rather than returning to a restored one.
      userNavigatedRef.current = true;
      setDraftKey(key);
      setDraftProviderId(null);
      setSheet(null);
      sheetTriggerRef.current?.focus({ preventScroll: true });
      return null;
    },
    [createWorkspace, setSelectedKey],
  );
  const handleResizeStart = useCallback(
    (event: ReactMouseEvent<HTMLButtonElement>) => startDrag("left", event),
    [startDrag],
  );
  const handleResizeKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLButtonElement>) => handleResizeKey("left", event),
    [handleResizeKey],
  );
  const handleToggleHistory = useCallback(() => {
    setHistoryWorkspace(null);
    setHistoryOpen((open) => !open);
  }, []);
  const handleHistorySearchChange = useCallback((value: string) => setHistorySearch(value), []);
  const handleOpenSettings = useCallback(() => selectSurface("settings"), [selectSurface]);
  const handleRetryProjects = useCallback(() => void retryProjects(), [retryProjects]);
  // The provider block retries the read that failed: re-reading projects would
  // leave the same sentence standing, and the sentence clears only when the
  // catalog answers. Only the latest retry may write it: a slower earlier one
  // must not undo a later answer, nor speak once this surface is gone.
  const providerRetryRef = useRef(0);
  useEffect(
    () => () => {
      providerRetryRef.current += 1;
    },
    [],
  );
  const handleRetryProviders = useCallback(async () => {
    const attempt = ++providerRetryRef.current;
    let failure: ErrorSentence | null = null;
    try {
      await loadChatProviders();
    } catch (cause: unknown) {
      failure = errorSentence(cause);
    }
    if (attempt === providerRetryRef.current) setProviderError(failure);
  }, [loadChatProviders]);
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
  // A click on an attention toast is this surface's own act, through the roads
  // a row click uses: the session opens as its tab, and a session the roster no
  // longer holds leaves its workspace in front instead of opening anything. The
  // opener is registered while this surface is mounted; the app-scope listener
  // falls back to the roster's own roads while it is not.
  const openAttentionTarget = useCallback(
    (target: AttentionTarget) => {
      const session = sharedSessionController()
        .getState()
        .sessions.find((row) => row.id === target.sessionId);
      if (session !== undefined) {
        handleReopenSession(session);
        return;
      }
      const key = target.workspaceId === null ? null : localWorkspaceKey(target.workspaceId);
      if (key !== null && knownWorkspaceKeys.has(key)) selectWorkspace(key);
      void refreshSessions();
    },
    [handleReopenSession, knownWorkspaceKeys, refreshSessions, selectWorkspace],
  );

  useEffect(() => {
    setAttentionOpener(openAttentionTarget);
    return () => setAttentionOpener(null);
  }, [openAttentionTarget]);

  const handleNewSession = useCallback(
    (trigger: HTMLButtonElement | null) => {
      // An agent belongs to a workspace: with none selected the provider flow
      // never opens, and the line says why instead.
      if (selectedKey === null) {
        setProviderError(NO_WORKSPACE_ERROR);
        return;
      }
      void chooseProvider({ kind: "strip" }, addSessionToWorkspace, trigger ?? undefined);
    },
    [addSessionToWorkspace, chooseProvider, selectedKey],
  );
  const dismissNewTabMenu = useCallback(() => setNewTabMenuOpen(false), []);
  // The menu's Agent entry: the flow the "+" owned before the menu, focused
  // from the "+" itself so a dismissed picker hands focus back to it.
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
  // — the address bar takes focus itself once the page is up. The entry waits
  // without a workspace, so this guard only answers a road that got past it:
  // it says why, never nothing.
  const handleNewTabBrowser = useCallback(() => {
    dismissNewTabMenu();
    if (selectedKey === null) {
      setProviderError(NO_WORKSPACE_ERROR);
      return;
    }
    openBrowserFor(selectedKey);
  }, [dismissNewTabMenu, openBrowserFor, selectedKey]);
  const pickProvider = useCallback((provider: ProviderInfo) => {
    setProviderPicker(null);
    setProviderAnchor(null);
    const afterChoice = afterProviderChoiceRef.current;
    afterProviderChoiceRef.current = null;
    afterChoice?.(provider);
  }, []);
  const dismissProviderPicker = useCallback(() => {
    afterProviderChoiceRef.current = null;
    // Escape and outside clicks: the choice ends with NO choice, which the
    // strip rule treats exactly like a failure — focus back to "+" if lost.
    noteChoiceDismissed();
    endProviderChoice();
    setProviderPicker(null);
    setProviderAnchor(null);
  }, [endProviderChoice, noteChoiceDismissed]);
  // The provider choice dismisses when the band opens, like every other menu:
  // the flow is transient and the band is the outside press.
  useMenuOpen(providerPicker !== null, dismissProviderPicker);
  // The empty picker's one action. Settings opens on its Providers tab, where
  // the install guidance lives; dismissing first ends the choice so the
  // navigate-away can never leave a flow running under the user left behind.
  const openProvidersSettings = useCallback(() => {
    dismissProviderPicker();
    selectSurface("settings");
  }, [dismissProviderPicker, selectSurface]);
  // A choice opened under one workspace must never create in another: a
  // pointer click on the row dismisses through the outside rule, but a
  // keyboard- or state-driven switch has no click to catch — the flow ends
  // here, in the workspace it was opened under, never the one now selected.
  useEffect(() => {
    if (!providerChoosing) return;
    if (choiceWorkspaceRef.current !== selectedKey) dismissProviderPicker();
  }, [providerChoosing, selectedKey, dismissProviderPicker]);
  useEffect(() => {
    if (providerAnchor === null) return;
    const onKey = (event: KeyboardEvent) => {
      if (isImeComposition(event)) return;
      if (event.key === "Escape") dismissProviderPicker();
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
        dismissProviderPicker();
      }
    };
    // Open over a viewport that then moved is stale: close it (the brief
    // picked closing over repositioning). Dismissing arms the focus rule,
    // so focus the unmount dropped comes back to "+".
    const onResize = () => {
      dismissProviderPicker();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onPointer);
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onPointer);
      window.removeEventListener("resize", onResize);
    };
  }, [dismissProviderPicker, providerAnchor]);
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
        // A generation change or an eviction replaces the attachment. The card
        // must adopt its subscription id so an answer reaches the live observer.
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
    dropPermissionRequest(sessionId, toolCallId);
    setPermissionQueue((queue) =>
      queue.filter(
        (item) => !(item.sessionId === sessionId && item.request.toolCallId === toolCallId),
      ),
    );
  }, []);
  // A card whose answer the daemon refused terminally: the item records it,
  // so every remount starts terminal instead of offering the answer again.
  // The registry lets go too, or a remount would re-seed the request fresh.
  const markPermissionStale = useCallback((sessionId: string, toolCallId: string) => {
    dropPermissionRequest(sessionId, toolCallId);
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
  useEffect(() => {
    const unsubscribe = subscribePermissions(
      ({ sessionId, subscriptionId, request }) =>
        handlePermissionRequest(sessionId, subscriptionId, request),
      handlePermissionResolved,
    );
    for (const { sessionId, subscriptionId, request } of pendingPermissionRequests()) {
      handlePermissionRequest(sessionId, subscriptionId, request);
    }
    return unsubscribe;
  }, [handlePermissionRequest, handlePermissionResolved]);

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
  // While a tool tab is in front its pane owns the centre: the hidden
  // session's card has no surface to render on. The gate reads the pane, not
  // the strip's selection — a tab in the pane below has left the pane above
  // free, and a waiting card must not disappear with it.
  // An answered plan is never a card again, whoever answered it: the transcript
  // row and the composer's icon carry it from the answer on.
  const cardStandsFor = (item: (typeof permissionQueue)[number]) =>
    item.request.kind !== "plan" || item.resolution === undefined;
  const selectedPermission =
    panes.upperTool !== null
      ? null
      : (permissionQueue.find(
          (item) => item.sessionId === selectedSessionId && item.resolution === undefined,
        ) ??
        permissionQueue.find(
          (item) => item.sessionId === selectedSessionId && cardStandsFor(item),
        ) ??
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

  // One instance of the provider choice UI, anchored where the flow was
  // opened. What happens afterwards was fixed when the flow started.
  // Memoised: the rail's bundle reads this node, and a fresh element every
  // render would defeat the rail's memo.
  const providerMenu = useMemo(
    () =>
      providerAnchor === null || providerPicker === null ? null : (
        <AnchoredPopover
          containerRef={providerPickerRef}
          anchorRef={providerAnchorElRef}
          onDismiss={dismissProviderPicker}
          className="workspace-surface-menu"
          role="listbox"
          aria-label="Choose agent"
        >
          <div className="workspace-menu-label">Choose agent</div>
          {providerPicker!.length === 0 ? (
            // The gate's empty state: no agent CLI is installed, so the flow
            // stops here instead of creating a session that cannot start.
            <div className="workspace-provider-empty">
              <p className="workspace-provider-empty-text">
                No agent CLI is installed on this machine. Install one — for example grok, claude,
                or gemini — then choose Refresh in Settings → Providers. Run on demand (npx) agents
                are listed in Settings → Providers too, and they are not offered here.
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
            <div className="workspace-provider-group">
              <div className="workspace-menu-label">Installed</div>
              <div className="workspace-surface-options">
                {providerPicker!.map((provider) => (
                  <button
                    type="button"
                    role="option"
                    className="workspace-surface-option"
                    key={provider.id}
                    onClick={() => pickProvider(provider)}
                  >
                    <span className="workspace-surface-name">{provider.id}</span>
                  </button>
                ))}
              </div>
            </div>
          )}
        </AnchoredPopover>
      ),
    [providerAnchor, providerPicker, dismissProviderPicker, openProvidersSettings, pickProvider],
  );

  // The daemon poll hands out a new object every 2 s; the bar reads only these
  // three fields, so an unchanged daemon keeps its identity and the bar's memo holds.
  const barDaemon = useMemo(
    () => daemon,
    // oxlint-disable-next-line react-hooks/exhaustive-deps -- the snapshot is the same object until one of these fields moves.
    [daemon.state, daemon.pid, daemon.message],
  );
  // The rail's bundle, stable across renders that change nothing it shows:
  // without this the inline literal below is a new identity every render
  // and the rail re-traverses its tree on every session and daemon update.
  const transcriptSearch: TranscriptSearchProps = useMemo(
    () => ({
      canSearch: transcriptSearchCanSearch,
      titleOf: (sessionId: string) => {
        const session = sessions.find((row) => row.id === sessionId);
        return session === undefined ? "Session" : sessionTitle(session);
      },
      onOpen: openTranscriptHit,
    }),
    [openTranscriptHit, sessions, transcriptSearchCanSearch],
  );
  const focusedAgentId = focusedAgent?.sessionId ?? null;
  const selectedAgents = useRailAgents(sessions, selectedKey, focusedAgentId);
  const openTasksTab = useCallback(() => {
    setActiveSidePanel("tasks");
    setRightCollapsed(false);
  }, [setRightCollapsed]);
  // The rail archives with the list's act. The roster is read from the controller
  // at each close, not from this render's array, so a prompt that outlives a new run
  // is refused. A closed child's tab goes with it; its transcript stays in History.
  const archiveAgent = useCallback(
    async (target: ArchiveTarget): Promise<string | null> => {
      const sentences = await archiveTargets(
        [target],
        () => sharedSessionController().getState().sessions,
        sessionClose,
      );
      const sentence = sentences.get(target.id);
      if (sentence !== undefined) return sentence;
      dropSessionPermissions(target.id);
      await refreshSessions();
      return null;
    },
    [refreshSessions, dropSessionPermissions],
  );
  const openHistoryForSelected = useCallback(() => {
    setHistoryWorkspace(selectedKey);
    setHistoryOpen(true);
  }, [selectedKey]);
  const sidebarTree: WorkspaceTreeProps = useMemo(
    () => ({
      projects: visibleProjects,
      loading: projectsLoading,
      error: projectsError,
      providerError,
      selectedWorkspace: selectedKey,
      onRetryProjects: handleRetryProjects,
      onRetryProviders: handleRetryProviders,
      onSelectWorkspace: selectWorkspace,
      onNewWorkspace: handleNewWorkspace,
      onRenameWorkspace: renameWorkspace,
      onArchiveWorkspace: handleArchiveWorkspace,
      onDeleteWorktree: handleDeleteWorktree,
      onRemoveProject: handleRemoveProject,
      providerMenuAnchorProjectId:
        providerAnchor?.kind === "project" ? providerAnchor.projectId : null,
      providerMenu: providerAnchor?.kind === "project" ? providerMenu : null,
      stats: workspaceStats,
      branches: workspaceBranches,
      hostNames: hostNameMap,
      hostOnline: hostOnlineMap,
      selectedAgents: {
        agents: selectedAgents,
        activeId: focusedAgentId,
        onOpenAgent: handleOpenOverviewSession,
        onOpenHistory: openHistoryForSelected,
        onArchiveAgent: archiveAgent,
      },
    }),
    [
      visibleProjects,
      projectsLoading,
      projectsError,
      providerError,
      selectedKey,
      handleRetryProjects,
      handleRetryProviders,
      selectWorkspace,
      handleNewWorkspace,
      renameWorkspace,
      handleArchiveWorkspace,
      handleDeleteWorktree,
      handleRemoveProject,
      providerAnchor,
      providerMenu,
      workspaceStats,
      workspaceBranches,
      hostNameMap,
      hostOnlineMap,
      selectedAgents,
      focusedAgentId,
      handleOpenOverviewSession,
      openHistoryForSelected,
      archiveAgent,
    ],
  );
  const firstPromptOnPane =
    draftShowing &&
    firstPrompt !== null &&
    paneSession !== null &&
    firstPrompt.sessionId === paneSession.id;
  const sendingPrompt =
    firstPrompt !== null && (firstPrompt.state === "queued" || firstPrompt.state === "sending");
  const sessionPaneNode =
    paneSession === null ? null : (
      <>
        {isAgentKind(paneSession.kind) ? (
          <AgentChatSurface
            key={paneSession.id}
            id={WORKSPACE_TERMINAL_PANEL_ID}
            sessionId={paneSession.id}
            fileLinks={chatFileLinks}
            loadFileIndex={loadPaneFileIndex}
            focusMessage={
              transcriptFocus?.sessionId === paneSession.id
                ? {
                    messageId: transcriptFocus.messageId,
                    nonce: transcriptFocus.nonce,
                  }
                : null
            }
            observedState={paneSession.state}
            resumeStopped={
              paneSession.resumable ? () => resumeStoppedSession(paneSession) : undefined
            }
            initialGoal={paneSession.goal}
            activity={paneSession.activity}
            daemonState={daemon.state}
            daemonInstanceId={daemon.instanceId}
            sessionRoster={sessions}
            onOpenSubagent={handleOpenSubagent}
            subagentAttention={subagentAttention}
            onRefreshSubagents={refreshSessions}
            onStopSubagent={stopAgent}
            onOpenTasks={openTasksTab}
            onAgentChange={reportPaneAgentWithFirstMessage}
            deviceNames={peerNames}
            hasPendingPermission={hasPendingPermission}
            pendingPlanToolCallId={pendingPlanToolCallId}
            // The daemon owns this session's follow-up queue (see
            // `queueSupported`): the surface renders its snapshots, and
            // Enter mid-turn queues instead of interrupting.
            queueSupported={queueSupported}
            gifWebpSupported={gifWebpSupported}
            fileUploadSupported={fileUploadSupported}
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
            // The recovered notice rides the header's own row instead of
            // adding one: banner, state word and Reopen share the pane
            // header on a recovered transcript, and a live pane shows
            // nothing (the bar renders null off a recovered state).
            headerTrailing={
              <RecoveredSessionBar
                key={`recovered-bar-${paneSession.id}`}
                session={paneSession}
                onReopened={handleReopenSession}
                onResumeFailed={handleResumeFailed}
              />
            }
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
    );
  // The first-message composer: the provider and the prompt, with the held text
  // kept until the agent confirms it.
  const draftComposer = (
    <DraftAgentComposer
      providers={draftProviders ?? []}
      providerId={draftProviderId}
      onProviderChange={pickDraftProvider}
      choices={draftChoices}
      selection={draftSelection}
      onSelectionChange={setDraftSelection}
      onSend={sendDraft}
      notice={firstPromptNotice}
      restore={
        firstPrompt !== null && firstPrompt.state === "handed"
          ? { text: firstPrompt.text, nonce: firstPrompt.attempt }
          : null
      }
      disabledReason={
        draftStarting
          ? "Starting the agent…"
          : sendingPrompt
            ? "Sending to the agent…"
            : draftProviders === null
              ? "Loading agents…"
              : draftProviders.length === 0
                ? "No agent is installed to start."
                : null
      }
    />
  );

  return (
    <section className="workspace-screen" data-screen-label="Workspace">
      <div className="workspace-panels">
        <Sidebar
          width={leftWidth}
          collapsed={leftCollapsed}
          onCollapsedChange={setLeftCollapsed}
          onResizeStart={handleResizeStart}
          onResizeKeyDown={handleResizeKeyDown}
          resizeMin={MIN_LEFT_WIDTH}
          resizeMax={leftMax}
          historyOpen={historyOpen}
          onToggleHistory={handleToggleHistory}
          searchValue={search}
          onSearchChange={handleSearchChange}
          onAddProject={openProjectDialog}
          onOpenSettings={handleOpenSettings}
          addProjectRef={newProjectTriggerRef}
          tree={sidebarTree}
          transcripts={transcriptSearch}
        />

        <main className="workspace-center-panel" ref={centerRef}>
          {/* The session subtree stays mounted behind the History page: an
              unsent composer draft and picker state survive the trip there
              and back. display:none drops it from the tab order and the
              accessibility tree while the page is up. */}
          <div className={historyOpen ? "workspace-center-hidden" : "workspace-center-live"}>
            {selectedRemoteHost === null ? null : (
              <RemoteWorkspaceSurface
                key={`${selectedRemoteHost.hostId}:${selectedRemoteHost.workspaceId}`}
                deviceId={selectedRemoteHost.hostId}
                workspaceId={selectedRemoteHost.workspaceId}
                hostOnline={hostOnlineMap.get(selectedRemoteHost.hostId) === true}
                onOpenFile={(path) => {
                  // A remote workspace's own key: the File tab reads and
                  // writes over the held link to that host.
                  const key = workspaceKey(
                    selectedRemoteHost.hostId,
                    selectedRemoteHost.workspaceId,
                  );
                  if (key !== null) openToolTab(key, path, "file");
                }}
              />
            )}
            {selectedRemoteHost === null ? (
              <>
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
                  overviewSessions={overviewSessions}
                  workspaceName={workspaceName}
                  onOpenSession={handleOpenOverviewSession}
                  selectedSessionId={selectedSessionId}
                  onMoveTab={moveStripTab}
                  onStripPointerDown={startTabDrag}
                />
                <SplitDragLayer
                  ref={dragLayerRef}
                  hasTab={hasLiveTab}
                  markAt={tabInsertionAt}
                  scrollStrip={scrollStripAt}
                  onDrop={dropTab}
                  boxes={() => ({
                    area: centerRef.current,
                    strip: document.querySelector<HTMLElement>(".workspace-session-tabs"),
                  })}
                />
                {/* What a pane act did, for a keyboard that cannot see the pane move. */}
                <div className="workspace-sr-only" role="status" aria-live="polite">
                  {paneAnnouncement}
                </div>
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
                          <span
                            id={`close-failure-${failure.id}-detail`}
                            className="error-detail-sr-only"
                          >
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
                (sessionsError.workspaceKey === null ||
                  sessionsError.workspaceKey === selectedKey) ? (
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

                <SplitPane
                  split={split}
                  onResize={resizeSplit}
                  onMerge={mergeSplit}
                  lowerLabel={lowerLabel}
                  lower={
                    lowerTool?.kind === "browser" ? (
                      <BrowserTab
                        key={lowerTool.id}
                        browserId={lowerTool.browserId}
                        url={browserRecordFor(lowerTool.browserId)?.url ?? BROWSER_START_URL}
                        workspaceKey={lowerTool.workspaceKey}
                      />
                    ) : null
                  }
                >
                  {paneTool?.kind === "browser" ? (
                    <BrowserTab
                      key={paneTool.id}
                      browserId={paneTool.browserId}
                      url={browserRecordFor(paneTool.browserId)?.url ?? BROWSER_START_URL}
                      workspaceKey={paneTool.workspaceKey}
                      onSplitDown={
                        lowerTabId === null && visibleSessions.length > 0
                          ? () => splitTabDown(paneTool)
                          : undefined
                      }
                    />
                  ) : paneTool !== null ? (
                    <div
                      id={WORKSPACE_TERMINAL_PANEL_ID}
                      className={`workspace-conversation workspace-scroll workspace-tool-pane${
                        paneTool.kind === "diff" ? " workspace-tool-pane-diff" : ""
                      }`}
                      role="tabpanel"
                      aria-label={paneTool.kind === "diff" ? "Diff" : "File"}
                    >
                      {paneTool.kind === "diff" ? (
                        <ToolDiffPane
                          key={paneTool.id}
                          workspaceKey={paneTool.workspaceKey}
                          path={paneTool.path}
                          refreshNonce={toolRefreshNonce}
                          cache={toolContentCache.diffs}
                        />
                      ) : (
                        <WorkspaceFileTab
                          key={paneTool.id}
                          workspaceKey={paneTool.workspaceKey}
                          path={paneTool.path}
                          refreshNonce={toolRefreshNonce}
                          cache={toolContentCache.fileCells}
                        />
                      )}
                    </div>
                  ) : paneSession !== null && !draftShowing ? (
                    sessionPaneNode
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
                      {draftShowing ? (
                        // One place for the composer whether or not the session is
                        // on the pane: a held prompt keeps its text across the switch.
                        <>
                          {firstPromptOnPane ? <div hidden>{sessionPaneNode}</div> : null}
                          {draftComposer}
                        </>
                      ) : sessionsLoading || projectsLoading ? (
                        <div role="status" className="workspace-empty-note">
                          Loading sessions…
                        </div>
                      ) : selectedKey === null && projects.length === 0 ? (
                        // No project at all: the one action is the same add-project
                        // flow the sidebar's "+" opens.
                        <div className="workspace-empty-state" role="status">
                          <p className="workspace-empty-title">No project yet</p>
                          <button
                            type="button"
                            className="workspace-empty-action"
                            onClick={openProjectDialog}
                          >
                            New project
                          </button>
                        </div>
                      ) : (
                        <div className="workspace-empty-state" role="status">
                          <p className="workspace-empty-title">No tabs yet</p>
                          {/* The spec's one outline action: the same agent flow as
                    "+ → Agent", from the "+" itself. It waits while a create or
                    a provider choice is in flight, and without a selected
                    workspace it says why. */}
                          <button
                            type="button"
                            className="workspace-empty-action"
                            onClick={handleNewTabAgent}
                            disabled={selectedKey === null || addDisabled}
                            aria-describedby={
                              selectedKey === null ? EMPTY_PANE_REASON_ID : undefined
                            }
                          >
                            Open an agent
                          </button>
                          {selectedKey === null ? (
                            <p id={EMPTY_PANE_REASON_ID} className="workspace-empty-note">
                              {NO_WORKSPACE_SENTENCE}
                            </p>
                          ) : null}
                        </div>
                      )}
                    </div>
                  )}
                </SplitPane>
              </>
            ) : null}
          </div>
          {historyOpen ? (
            <HistoryPanel
              search={historySearch}
              onRestored={handleRetryProjects}
              onSearchChange={handleHistorySearchChange}
              projects={projects}
              branches={workspaceBranches}
              onWorkspaceKeysChange={handleHistoryWorkspaceKeys}
              selectedSessionId={panes.upperToolId === null ? selectedSessionId : null}
              onReopen={handleReopenSession}
              onReopenAgent={handleReopenAgent}
              hosts={historyHosts}
              hostFilter={historyHost}
              workspaceFilter={historyWorkspace}
              onHostFilterChange={setHistoryHost}
            />
          ) : null}
        </main>

        {/* The History page owns the main area: the side panel and its
            handle step out of the layout while it is up — mounted, so the
            panel comes back exactly as it was left, tab and all. */}
        <button
          type="button"
          className={`workspace-resize-handle${historyOpen ? " workspace-panel-hidden" : ""}`}
          onMouseDown={(event) => startDrag("right", event)}
          onDoubleClick={() => setRightCollapsed((collapsed) => !collapsed)}
          onKeyDown={(event) => handleResizeKey("right", event)}
          title="Drag to resize · double-click to collapse"
          aria-label="Resize side panel"
          aria-orientation="vertical"
          aria-valuemin={MIN_RIGHT_WIDTH}
          aria-valuemax={rightMax}
          aria-valuenow={rightWidth}
        />

        <aside
          className={`workspace-panel workspace-right-panel${
            historyOpen ? " workspace-panel-hidden" : ""
          }`}
          style={{ width: rightCollapsed ? "30px" : `${rightWidth}px` }}
          aria-label="Workspace side panel"
        >
          {rightCollapsed ? (
            <PanelStub side="right" onShow={() => setRightCollapsed(false)} />
          ) : (
            <div className="workspace-panel-open">
              <SidePanelTabs
                registry={sidePanelRegistry}
                activeId={activeSidePanel}
                onSelect={setActiveSidePanel}
                onCollapse={() => setRightCollapsed(true)}
                panelWidth={rightWidth}
                attentionIds={tasksUnseen && !tasksVisible ? ["tasks"] : undefined}
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
                  selectedSurface.placement === "tab"
                    ? sidePanelTabId(selectedSurface.id)
                    : undefined
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
                      agentTasks,
                    })}
                  </ConfirmProvider>
                </SurfaceErrorBoundary>
              </div>
            </div>
          )}
        </aside>
      </div>
      <StatusBar
        agent={focusedAgent}
        daemon={barDaemon}
        daemonNote={restartFailureNote}
        progress={statusProgress}
      />

      {sheet === null ? null : (
        <NewWorkspaceSheet
          key={sheet.nonce}
          projects={sheetProjects}
          presetProjectId={sheet.projectId}
          pairedHosts={pairedHostsForSheet}
          onCreate={createFromSheet}
          onClose={closeSheet}
        />
      )}

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
