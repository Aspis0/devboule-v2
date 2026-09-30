import { useCallback, useEffect, useRef, useState } from "react";
import type { RefObject } from "react";
import type { DesignHost } from "./designHost";
import type { WorkspaceProject } from "./designSurfaceTypes";
import type { ProviderInfo, Workspace } from "../../types/ipc";
import { chatCapableProviders } from "../workspace/workspaceSessions";
import {
  loadDesignProviderId,
  loadDesignWorkspaceId,
  loadStoredDesignProviderId,
  loadStoredDesignWorkspaceId,
  saveDesignProviderId,
  saveDesignWorkspaceId,
} from "./designSettings";
import { projectsList, providersList, workspacesList } from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";

const WORKSPACE_NOT_REGISTERED_NOTICE = "The attached folder is no longer registered.";
const WORKSPACE_UNCONFIRMED_NOTICE =
  "The attached folder could not be confirmed because its record failed to load.";

interface UseDesignWorkspaceInput {
  host: DesignHost;
  busy: boolean;
  mountedRef: RefObject<boolean>;
  reportPersistence: (
    kind: "provider" | "workspace" | "skill" | "output" | "history",
    saved: boolean,
  ) => void;
}

interface UseDesignWorkspaceResult {
  providers: ProviderInfo[];
  providersLoading: boolean;
  selectedProviderId: string | null;
  unavailableProviderId: string | null;
  workspaceProjects: WorkspaceProject[];
  workspacesLoading: boolean;
  workspacesRefreshing: boolean;
  workspacesError: ErrorSentence | null;
  selectedWorkspaceId: string | null;
  workspaceSelectionNotice: string | null;
  workspaceSelectionUnresolved: boolean;
  refreshWorkspaceProjects: (initialLoad: boolean, isActive?: () => boolean) => Promise<void>;
  selectProvider: (provider: ProviderInfo) => void;
  selectWorkspace: (workspace: Workspace | null) => void;
  openWorkspacePicker: () => void;
}

export function useDesignWorkspace(input: UseDesignWorkspaceInput): UseDesignWorkspaceResult {
  // Destructured (not `input.*`): exhaustive-deps cannot resolve member reads
  // inside the nested `.then` closures below and demands the whole `input`
  // object, which is a fresh literal every render and would recreate every
  // callback per render. These four fields are stable-or-same-firing (host
  // prop, busy boolean, mounted ref object, stable report callback), so the
  // bodies and dep lists below stay byte-identical to the component version.
  const { host, busy, mountedRef, reportPersistence } = input;
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [providersLoading, setProvidersLoading] = useState(true);
  const [selectedProviderId, setSelectedProviderId] = useState<string | null>(null);
  const [unavailableProviderId, setUnavailableProviderId] = useState<string | null>(null);
  const [workspaceProjects, setWorkspaceProjects] = useState<WorkspaceProject[]>([]);
  const [workspacesLoading, setWorkspacesLoading] = useState(true);
  const [workspacesRefreshing, setWorkspacesRefreshing] = useState(false);
  const [workspacesError, setWorkspacesError] = useState<ErrorSentence | null>(null);
  const [selectedWorkspaceId, setSelectedWorkspaceId] = useState<string | null>(null);
  const [workspaceSelectionNotice, setWorkspaceSelectionNotice] = useState<string | null>(null);
  const [workspaceSelectionUnresolved, setWorkspaceSelectionUnresolved] = useState(false);
  const providerSelectionInteractedRef = useRef(false);
  const workspaceSelectionInteractedRef = useRef(false);
  const workspaceSelectionIdRef = useRef<string | null>(null);
  const workspaceSelectionUnresolvedRef = useRef(false);
  const workspaceRequestTokenRef = useRef(0);

  const updateWorkspaceSelection = useCallback(
    (workspaceId: string | null, unresolved: boolean, notice: string | null): void => {
      workspaceSelectionIdRef.current = workspaceId;
      workspaceSelectionUnresolvedRef.current = unresolved;
      setSelectedWorkspaceId(workspaceId);
      setWorkspaceSelectionUnresolved(unresolved);
      setWorkspaceSelectionNotice(notice);
    },
    [],
  );

  useEffect(() => {
    let active = true;
    void providersList()
      .then((catalog) => {
        if (!active) return;
        const available = chatCapableProviders(catalog.providers);
        setProviders(available);
        return loadDesignProviderId(available.map((provider) => provider.id)).then((storedId) => {
          if (!active) return;
          if (providerSelectionInteractedRef.current) return;
          setSelectedProviderId(storedId);
          setUnavailableProviderId(null);
          if (storedId !== null) {
            const storedProvider = available.find((provider) => provider.id === storedId);
            if (storedProvider !== undefined) {
              // Mount restores the preference only; the first generation owns session creation.
              (host.setProviderPreference ?? host.selectProvider)?.(storedProvider);
            }
            return;
          }
          return loadStoredDesignProviderId().then((rawStoredId) => {
            if (!active || providerSelectionInteractedRef.current) return;
            if (
              rawStoredId !== null &&
              !available.some((provider) => provider.id === rawStoredId)
            ) {
              setUnavailableProviderId(rawStoredId);
            }
          });
        });
      })
      .catch(() => {
        if (active) {
          setProviders([]);
          setSelectedProviderId(null);
          setUnavailableProviderId(null);
        }
      })
      .finally(() => {
        if (active) setProvidersLoading(false);
      });
    return () => {
      active = false;
    };
  }, [host]);

  const refreshWorkspaceProjects = useCallback(
    async (initialLoad: boolean, isActive: () => boolean = () => true): Promise<void> => {
      const requestToken = ++workspaceRequestTokenRef.current;
      if (initialLoad) setWorkspacesLoading(true);
      else setWorkspacesRefreshing(true);
      setWorkspacesError(null);

      // active only rejects updates after unmount; opening twice can leave an older response alive
      // while a newer request is current, so the token also orders concurrent refreshes.
      const isCurrent = (): boolean =>
        isActive() && mountedRef.current && requestToken === workspaceRequestTokenRef.current;

      try {
        const projects = await projectsList();
        const records = await Promise.all(
          projects.map(async (project): Promise<WorkspaceProject> => {
            try {
              return { ...project, workspaces: await workspacesList(project.id) };
            } catch {
              return {
                ...project,
                workspaces: [],
                workspaceError: "Workspaces could not be loaded.",
              };
            }
          }),
        );
        if (!isCurrent()) return;

        setWorkspaceProjects(records);
        const workspaceIds = records.flatMap((project) =>
          project.workspaces.map((workspace) => workspace.id),
        );
        const failedProjectExists = records.some((project) => project.workspaceError !== undefined);
        const existingSession = host.getAgentSessionRecord?.() ?? null;
        const wasUnresolved = workspaceSelectionUnresolvedRef.current;
        let storedSelection = false;
        let candidateId: string | null;

        if (existingSession !== null) {
          candidateId = existingSession.workspaceId;
        } else if (!initialLoad || workspaceSelectionInteractedRef.current) {
          candidateId = workspaceSelectionIdRef.current;
        } else {
          storedSelection = true;
          candidateId = failedProjectExists
            ? await loadStoredDesignWorkspaceId()
            : await loadDesignWorkspaceId(workspaceIds);
          if (!isCurrent()) return;
          if (workspaceSelectionInteractedRef.current) {
            candidateId = workspaceSelectionIdRef.current;
            storedSelection = false;
          }
        }

        if (!isCurrent()) return;
        const selectedWorkspace =
          candidateId === null
            ? undefined
            : records
                .flatMap((project) => project.workspaces)
                .find((workspace) => workspace.id === candidateId);

        if (candidateId !== null && selectedWorkspace !== undefined) {
          updateWorkspaceSelection(candidateId, false, null);
          if (existingSession === null && (storedSelection || (!initialLoad && wasUnresolved))) {
            (host.setWorkspacePreference ?? host.selectWorkspace)?.(selectedWorkspace);
          }
        } else if (candidateId !== null && failedProjectExists) {
          updateWorkspaceSelection(candidateId, true, WORKSPACE_UNCONFIRMED_NOTICE);
          if (!initialLoad && !wasUnresolved) {
            (host.setWorkspacePreference ?? host.selectWorkspace)?.(null);
          }
        } else if (!initialLoad && candidateId !== null) {
          updateWorkspaceSelection(null, false, WORKSPACE_NOT_REGISTERED_NOTICE);
          (host.setWorkspacePreference ?? host.selectWorkspace)?.(null);
          void saveDesignWorkspaceId(null).then((saved) => reportPersistence("workspace", saved));
        } else if (candidateId !== null || initialLoad) {
          updateWorkspaceSelection(null, false, null);
        }
      } catch (cause: unknown) {
        if (!isCurrent()) return;
        const mapped = errorSentence(cause);
        setWorkspacesError({
          sentence: `Could not load workspaces: ${mapped.sentence}`,
          detail: mapped.detail,
        });
      }
      if (!isCurrent()) return;
      if (initialLoad) setWorkspacesLoading(false);
      else {
        setWorkspacesRefreshing(false);
        setWorkspacesLoading(false);
      }
    },
    [host, mountedRef, reportPersistence, updateWorkspaceSelection],
  );

  useEffect(() => {
    let active = true;
    void refreshWorkspaceProjects(true, () => active);
    return () => {
      active = false;
    };
  }, [refreshWorkspaceProjects]);

  const selectProvider = useCallback(
    (provider: ProviderInfo) => {
      if (busy) return;
      providerSelectionInteractedRef.current = true;
      setSelectedProviderId(provider.id);
      setUnavailableProviderId(null);
      (host.setProviderPreference ?? host.selectProvider)?.(provider);
      void saveDesignProviderId(provider.id).then((saved) => reportPersistence("provider", saved));
    },
    [busy, host, reportPersistence],
  );
  const selectWorkspace = useCallback(
    (workspace: Workspace | null) => {
      if (busy) return;
      workspaceSelectionInteractedRef.current = true;
      updateWorkspaceSelection(workspace?.id ?? null, false, null);
      (host.setWorkspacePreference ?? host.selectWorkspace)?.(workspace);
      const workspaceId = workspace?.id ?? null;
      void saveDesignWorkspaceId(workspaceId).then((saved) =>
        reportPersistence("workspace", saved),
      );
    },
    [busy, host, reportPersistence, updateWorkspaceSelection],
  );
  const openWorkspacePicker = useCallback(() => {
    if (busy) return;
    void refreshWorkspaceProjects(false);
  }, [busy, refreshWorkspaceProjects]);

  return {
    providers,
    providersLoading,
    selectedProviderId,
    unavailableProviderId,
    workspaceProjects,
    workspacesLoading,
    workspacesRefreshing,
    workspacesError,
    selectedWorkspaceId,
    workspaceSelectionNotice,
    workspaceSelectionUnresolved,
    refreshWorkspaceProjects,
    selectProvider,
    selectWorkspace,
    openWorkspacePicker,
  };
}
