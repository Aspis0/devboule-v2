import {
  useEffect,
  useRef,
  useState,
  type Dispatch,
  type RefObject,
  type SetStateAction,
} from "react";
import { providerUpdate, providersList } from "../../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import { useAppStore } from "../../../store/appStore";
import { sharedSessionController } from "../../workspace/workspaceSessions";
import { requestTerminalInput } from "../../terminal/pendingTerminalInput";
import { getLastSelectedWorkspaceKey } from "../../workspace/lastSelectedWorkspace";
import { parseWorkspaceKey } from "../../workspace/hosts/hostIdentity";
import { logTail } from "../providerStatus";
import { recordTerminalRun } from "./providerTerminalRuns";
import { fetchTerminalShell } from "./terminalShell";
import {
  providerInstallPlan,
  providerLoginPlan,
  type ProviderShell,
} from "./providerTerminalCommands";
import type { ProviderCatalog, ProviderInfo } from "../../../types/ipc";
import type { ProviderRowConsent } from "./ProviderRow";
import type { useProviderSwitches } from "./useProviderSwitches";
import {
  NO_WORKSPACE_INSTALL,
  NPM_MISSING,
  NPM_WARNING,
  SHELL_UNKNOWN,
  TERMINAL_LEAD,
  copyPlanLines,
  headlessLine,
  installNote,
  mergeAuthChecks,
} from "./providerPanelCopy";

/** A pending npm run on one provider row: what the daemon is doing right now. */
export interface ProviderNpmRun {
  providerId: string;
  verb: "update" | "install";
}

/** A provider held open in the consent card, waiting for the user's Confirm. */
export interface ProviderConsent {
  provider: ProviderInfo;
  verb: "update" | "install" | "login";
}

/** A failure a row or the page must show until dismissed. */
export interface ProviderFailure {
  providerId: string;
  text: string;
  detail: string | null;
}

type ShellQuery =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; shell: ProviderShell }
  | { status: "unknown" };

/**
 * Install, update and login for the Providers page: the consent card, the one
 * npm run the daemon executes, and the terminal handoff. Results land in the
 * catalogue through the setters the catalogue hook returns.
 */
export function useProviderConsent({
  catalog,
  panelRef,
  switches,
  setCatalog,
  setError,
  fetchSeqRef,
  invalidateModelCounts,
  onRunsChanged,
}: {
  catalog: ProviderCatalog | null;
  panelRef: RefObject<HTMLDivElement | null>;
  switches: Pick<ReturnType<typeof useProviderSwitches>, "beginFetch" | "reconcile">;
  setCatalog: Dispatch<SetStateAction<ProviderCatalog | null>>;
  setError: Dispatch<SetStateAction<ErrorSentence | null>>;
  fetchSeqRef: { current: number };
  invalidateModelCounts: () => void;
  /** The page's handoff notes changed: the page re-renders its row lines. */
  onRunsChanged: () => void;
}) {
  const [npmRun, setNpmRun] = useState<ProviderNpmRun | null>(null);
  const [npmFailure, setNpmFailure] = useState<ProviderFailure | null>(null);
  const [consent, setConsent] = useState<ProviderConsent | null>(null);
  // Which shell new tabs run, from the daemon's own OS report. Fetched on
  // consent open (install only — login lines are shell-independent); the
  // consent waits for it, and an unknown shell means copy, never type.
  const [shellQuery, setShellQuery] = useState<ShellQuery>({ status: "idle" });
  // A consent closed or replaced while its shell fetch is in flight must
  // not apply the stale answer to whatever opened next.
  const shellQuerySeq = useRef(0);
  // A terminal tab the daemon would not start: panel-level, because the
  // row that asked may belong to either section. The daemon's own reason
  // comes back through the same shared-controller store the creating lock
  // is read from — never invented here.
  const [terminalError, setTerminalError] = useState<ProviderFailure | null>(null);
  // Cleared after the close commits (not at the end of confirm): a second
  // synchronous click still sees the stale non-null consent, so the ref must
  // stay armed until that re-render.
  const consentInFlightRef = useRef(false);
  const consentRestoreRef = useRef<HTMLButtonElement | null>(null);
  // Set on Confirm: the effect below moves focus onto the row showing the
  // npm run. Found by attribute at focus time — no node registry, no
  // render-phase ref access.
  const pendingFocusRowRef = useRef<string | null>(null);

  // The workspace a terminal handoff opens under: the "+" menu's own rule
  // is that a terminal without one starts in the daemon's directory, so
  // without one this page offers no tab at all (headless install instead).
  const terminalWorkspaceKey = getLastSelectedWorkspaceKey();
  // What the daemon is sent for that handoff: the id, never the UI's key.
  const terminalWorkspaceId =
    terminalWorkspaceKey === null ? null : parseWorkspaceKey(terminalWorkspaceKey).workspaceId;
  const selectSurface = useAppStore((state) => state.selectSurface);
  const { beginFetch, reconcile } = switches;

  useEffect(() => {
    if (npmRun === null) {
      pendingFocusRowRef.current = null;
      return;
    }
    const providerId = pendingFocusRowRef.current;
    if (providerId === null) return;
    const node = [...(panelRef.current?.querySelectorAll("[data-provider-row]") ?? [])].find(
      (element) => element.getAttribute("data-provider-row") === providerId,
    ) as HTMLElement | undefined;
    node?.focus();
    if (node) pendingFocusRowRef.current = null;
  }, [npmRun, catalog, panelRef]);

  useEffect(() => {
    consentInFlightRef.current = false;
    if (consent === null) {
      consentRestoreRef.current?.focus();
      consentRestoreRef.current = null;
    }
  }, [consent]);

  function openConsent(
    provider: ProviderInfo,
    verb: "update" | "install" | "login",
    trigger: HTMLButtonElement | null,
  ) {
    if (npmRun !== null) return;
    consentRestoreRef.current = trigger;
    setConsent({ provider, verb });
    if (verb === "install" && terminalWorkspaceId !== null) {
      const seq = ++shellQuerySeq.current;
      setShellQuery({ status: "loading" });
      void fetchTerminalShell().then((shell) => {
        if (seq !== shellQuerySeq.current) return;
        setShellQuery(shell === null ? { status: "unknown" } : { status: "ready", shell });
      });
    } else {
      setShellQuery({ status: "idle" });
    }
  }

  function confirmConsent() {
    if (consent === null || consentInFlightRef.current) return;
    consentInFlightRef.current = true;
    // Terminal handoff when a workspace can host the tab and — for an
    // install — the shell is known (loading Confirm is disabled, so
    // reaching here mid-fetch is a dead branch). Headless daemon npm
    // otherwise (update always, install when no workspace is open — the
    // daemon needs no cwd). A login without a workspace has no entry
    // points, so reaching here with one is a dead branch, never a send.
    if (consent.verb === "install" && terminalWorkspaceId !== null) {
      const { provider } = consent;
      if (shellQuery.status === "loading" || shellQuery.status === "idle") return;
      // Without a confirmed shell the tab opens untyped: the person
      // pastes the copied line themselves (copy-mode consent above).
      const handoff =
        shellQuery.status === "ready"
          ? { shell: shellQuery.shell, typed: true }
          : { shell: null, typed: false };
      setConsent(null);
      confirmTerminal(provider, "install", terminalWorkspaceId, handoff);
      return;
    }
    if (consent.verb === "login") {
      // Login lines are static words with no shell syntax, so an unknown
      // shell never blocks them — only a missing workspace does.
      if (terminalWorkspaceId === null) {
        setConsent(null);
        return;
      }
      const { provider } = consent;
      setConsent(null);
      confirmTerminal(provider, "login", terminalWorkspaceId, { shell: null, typed: true });
      return;
    }
    const { provider } = consent;
    // Confirm always lands focus on the row showing the npm run: the
    // kebab trigger is already unmounted, and the details Update button
    // unmounts under the actions lock. The restore effect keeps serving
    // Cancel/Escape, whose triggers stay mounted.
    pendingFocusRowRef.current = provider.id;
    setConsent(null);
    setNpmFailure(null);
    setNpmRun({ providerId: provider.id, verb: consent.verb });
    // Sequence for the post-success refetch; a concurrent refresh supersedes it.
    const seq = ++fetchSeqRef.current;
    void providerUpdate(provider.id)
      .then((outcome) => {
        if (!outcome.ok) {
          setNpmFailure({ providerId: provider.id, text: logTail(outcome.log), detail: null });
          return;
        }
        // A version bump can change the model list: counts re-read on expand.
        invalidateModelCounts();
        // The refetch is the proof: the fresh catalog carries the new version.
        const switchFetch = beginFetch();
        void providersList()
          .then((fresh) => {
            if (seq === fetchSeqRef.current) {
              reconcile(fresh.providers, switchFetch);
              setCatalog((current) => (current === null ? fresh : mergeAuthChecks(fresh, current)));
            }
          })
          .catch((cause: unknown) => {
            if (seq === fetchSeqRef.current) setError(errorSentence(cause));
          });
      })
      .catch((cause: unknown) => {
        const mapped = errorSentence(cause);
        setNpmFailure({ providerId: provider.id, text: mapped.sentence, detail: mapped.detail });
      })
      // Unconditional: setState on an unmounted component is a safe no-op in
      // React 18+, and an unmount guard wedged the Refresh button once under
      // StrictMode.
      .finally(() => {
        setNpmRun(null);
      });
  }

  /**
   * Install and login run in a terminal tab under the current workspace —
   * never in the daemon's directory (the "+" menu refuses that create,
   * and so does this page). The login is interactive (browser, TUI),
   * which the headless npm road cannot do. An untyped handoff (unknown
   * shell) opens the tab and records both lines for the paste note.
   */
  function confirmTerminal(
    provider: ProviderInfo,
    verb: "install" | "login",
    workspaceId: string,
    handoff: { shell: ProviderShell | null; typed: boolean },
  ) {
    const plan =
      verb === "install" && handoff.shell !== null
        ? providerInstallPlan(provider, handoff.shell)
        : verb === "login"
          ? providerLoginPlan(provider)
          : null;
    if (handoff.typed && plan === null) return;
    // The controller drops a create while one is in flight without asking
    // the daemon: say that plainly, never the daemon-refusal sentence.
    if (sharedSessionController().getState().creating) {
      setTerminalError({
        providerId: provider.id,
        text: "A terminal is already starting — wait a moment and try again.",
        detail: null,
      });
      return;
    }
    void sharedSessionController()
      .create("terminal", null, workspaceId)
      .then((session) => {
        if (session === null) {
          // A genuine refusal: the controller published the daemon's own
          // words on the same store — read them, never invent them.
          const refusal = sharedSessionController().getState().error;
          setTerminalError({
            providerId: provider.id,
            text: refusal
              ? `${provider.id}: ${refusal.sentence}`
              : "Could not open a terminal tab. The daemon did not start one — try again.",
            detail: refusal?.detail ?? null,
          });
          return;
        }
        // Copy-mode lines for the paste note, when the tab opens untyped.
        const copyLines =
          !handoff.typed && verb === "install" ? (copyPlanLines(provider) ?? []) : [];
        if (handoff.typed && plan !== null) requestTerminalInput(session.id, plan.lines);
        recordTerminalRun(
          provider.id,
          verb,
          handoff.typed && plan !== null
            ? plan.lines.map((text) => ({ label: null, text }))
            : copyLines,
          session.id,
          { typed: handoff.typed },
        );
        onRunsChanged();
        setTerminalError(null);
        // create() already selected the tab; the surface switch remounts
        // this panel, so the handoff note lives in the module store. And
        // only while this panel is still the surface: a slow create must
        // not yank the person back from where they went meanwhile.
        if (useAppStore.getState().activeSurface === "settings") selectSurface("workspace");
      });
  }

  /**
   * What the open consent shows for this row: the headless line, the gated
   * terminal line, both copy lines when the shell is unknown, or the
   * waiting marker while the shell report is in flight. Null renders
   * nothing (unreachable: entry points already checked the same facts).
   */
  function consentView(
    provider: ProviderInfo,
    verb: "update" | "install" | "login",
  ): ProviderRowConsent | "waiting" | null {
    if (verb === "update") {
      const line = headlessLine(provider);
      return line === null ? null : { verb, lines: [line], copyLines: null, notice: NPM_WARNING };
    }
    if (verb === "login") {
      // Login lines are static words with no shell syntax: no fetch, and
      // an unknown shell never blocks them.
      const plan = providerLoginPlan(provider);
      return plan === null
        ? null
        : {
            verb,
            lines: plan.lines,
            copyLines: null,
            notice: plan.note ? `${TERMINAL_LEAD} ${plan.note}` : TERMINAL_LEAD,
          };
    }
    if (terminalWorkspaceId === null) {
      const line = headlessLine(provider);
      return line === null
        ? null
        : {
            verb,
            lines: [line],
            copyLines: null,
            notice: `${NO_WORKSPACE_INSTALL} ${NPM_WARNING}`,
          };
    }
    if (shellQuery.status === "loading" || shellQuery.status === "idle") return "waiting";
    const note = installNote(provider);
    if (shellQuery.status === "unknown") {
      const entries = copyPlanLines(provider);
      // A terminal handoff names the tab first, then the npm change the
      // pasted line makes, then the profile caveat the PTY reintroduces.
      return entries === null
        ? null
        : {
            verb,
            lines: [],
            copyLines: entries,
            notice: `${SHELL_UNKNOWN} ${NPM_WARNING} ${NPM_MISSING}${note ? ` ${note}` : ""}`,
          };
    }
    const plan = providerInstallPlan(provider, shellQuery.shell);
    return plan === null
      ? null
      : {
          verb,
          lines: plan.lines,
          copyLines: null,
          notice: `${TERMINAL_LEAD} ${NPM_WARNING} ${NPM_MISSING}${note ? ` ${note}` : ""}`,
        };
  }

  return {
    npmRun,
    npmFailure,
    consent,
    terminalError,
    terminalWorkspaceId,
    openConsent,
    confirmConsent,
    consentView,
    closeConsent: () => setConsent(null),
    dismissFailure: () => setNpmFailure(null),
    dismissTerminalError: () => setTerminalError(null),
  };
}
