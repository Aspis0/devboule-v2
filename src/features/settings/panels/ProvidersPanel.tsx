import { useEffect, useMemo, useRef, useState } from "react";
import {
  providerUpdate,
  providersAuthCheck,
  providersList,
  providersRefresh,
} from "../../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import { ErrorText } from "../../../components/ErrorText";
import { useAppStore } from "../../../store/appStore";
import { sharedSessionController } from "../../workspace/workspaceSessions";
import { hasTerminalInput, requestTerminalInput } from "../../terminal/pendingTerminalInput";
import { useSettingsDaemon } from "../settingsDaemon";
import {
  PROVIDER_SWITCHES_CAPABILITY,
  PROVIDER_AUTH_CHECK_CAPABILITY,
  TOOL_POLICY_CAPABILITY,
  logTail,
  toolPolicyFor,
} from "../providerStatus";
import { ProviderConsentBlock } from "../providers/ProviderConsentBlock";
import type { ProviderCatalog, ProviderInfo } from "../../../types/ipc";
import {
  PROVIDER_VOCABULARY_CAPABILITY,
  type ModelCountCache,
} from "../providers/ProviderModelCount";
import {
  SHELL_LABELS,
  providerInstallPackage,
  providerInstallPlan,
  providerLogin,
  providerLoginPlan,
  providerNoLoginNote,
  type ProviderShell,
} from "../providers/providerTerminalCommands";
import { SHELL_QUERY_LOADING, fetchTerminalShell } from "../providers/terminalShell";
import { getLastSelectedWorkspaceId } from "../../workspace/lastSelectedWorkspace";
import {
  TERMINAL_TAKE_TIMEOUT_MS,
  clearTerminalRun,
  clearTerminalRuns,
  recordTerminalRun,
  terminalRunDisplay,
  terminalRuns,
  type ProviderTerminalRun,
} from "../providers/providerTerminalRuns";
import { CopyableLines, type CopyableLine } from "../providers/CopyableLines";
import { ProviderNpmFailure } from "../providers/ProviderNpmFailure";
import {
  ProviderRow,
  ProviderVersionLine,
  type ProviderRowConsent,
} from "../providers/ProviderRow";
import { useToolPolicies } from "../providers/useToolPolicies";
import { ToolPolicyBanner } from "../providers/ToolPolicyBanner";
import { useProviderSwitches } from "../providers/useProviderSwitches";
import "../providers.css";

/** A pending npm run on one provider row: what the daemon is doing right now. */
interface ProviderNpmRun {
  providerId: string;
  verb: "update" | "install";
}

/** A provider held open in the consent card, waiting for the user's Confirm. */
interface ProviderConsent {
  provider: ProviderInfo;
  verb: "update" | "install" | "login";
}

function mergeAuthChecks(catalog: ProviderCatalog, checked: ProviderCatalog): ProviderCatalog {
  const checks = new Map(checked.providers.map((provider) => [provider.id, provider]));
  return {
    ...catalog,
    providers: catalog.providers.map((provider) => {
      const check = checks.get(provider.id);
      return check?.authStatus == null
        ? provider
        : {
            ...provider,
            authStatus: check.authStatus,
            authReason: check.authReason,
            authCheckedAt: check.authCheckedAt,
          };
    }),
  };
}

/** The consent's own words: what Confirm types, and what it changes. */
const NPM_WARNING =
  "This changes your global npm installation; running sessions keep the old version until they are restarted.";
const TERMINAL_LEAD = "Confirm opens a terminal tab and types this line, then takes you there.";
// The PTY starts `-NoProfile`, so a profile-provided npm is absent here —
// the daemon road's one honest sentence, restored where it can now happen.
const NPM_MISSING =
  "If the tab says npm is not recognized, install Node.js/npm and try again — the tab starts without your shell profile.";
const NO_WORKSPACE_INSTALL =
  "No workspace is open, so this installs in the background with no login step — open a workspace afterwards, then use Log in on the installed row.";
// Shown when the shell cannot be confirmed: the page must not auto-type.
const SHELL_UNKNOWN =
  "The terminal's shell could not be confirmed — copy the line for your shell. Confirm opens the tab for you to paste into.";

/**
 * The row line after a terminal handoff, until a successful Refresh or
 * dismiss. Handoff truth only: nothing here observes the install, so
 * nothing here may claim it is running.
 */
function terminalRunNotice(verb: "install" | "login"): string {
  return verb === "install"
    ? "Install and login sent to a terminal tab — finish them there."
    : "Login sent to a terminal tab — finish it there.";
}

/**
 * The Providers page: an Installed card of h44 rows (chevron details, glyph,
 * status, one Devboule-tools switch, kebab) and an Available to install card
 * with catalogue search and accent Install buttons. Install and Update share
 * one consent card and one npm-failure block; the model count behind each
 * Ready status is lazy (see `ProviderModelCount`).
 */
export function ProvidersPanel() {
  const [catalog, setCatalog] = useState<ProviderCatalog | null>(null);
  const [error, setError] = useState<ErrorSentence | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [query, setQuery] = useState("");
  // Bumped by every fetch (mount and refresh); a response only applies when its
  // sequence is still the latest, so a slow mount list cannot revert a refresh.
  const fetchSeqRef = useRef(0);
  const authCheckDoneEpochRef = useRef(-1);
  // Set synchronously on click so a second click before the re-render is a no-op.
  const refreshInFlightRef = useRef(false);
  // The one npm run the daemon is executing on this client's behalf.
  const [npmRun, setNpmRun] = useState<ProviderNpmRun | null>(null);
  // Per-row, dismissible failure from the last npm run.
  const [npmFailure, setNpmFailure] = useState<{
    providerId: string;
    text: string;
    detail: string | null;
  } | null>(null);
  const [consent, setConsent] = useState<ProviderConsent | null>(null);
  // Which shell new tabs run, from the daemon's own OS report. Fetched on
  // consent open (install only — login lines are shell-independent); the
  // consent waits for it, and an unknown shell means copy, never type.
  const [shellQuery, setShellQuery] = useState<
    | { status: "idle" }
    | { status: "loading" }
    | { status: "ready"; shell: ProviderShell }
    | { status: "unknown" }
  >({ status: "idle" });
  // A consent closed or replaced while its shell fetch is in flight must
  // not apply the stale answer to whatever opened next.
  const shellQuerySeq = useRef(0);
  // A terminal tab the daemon would not start: panel-level, because the
  // row that asked may belong to either section. The daemon's own reason
  // comes back through the same shared-controller store the creating lock
  // is read from — never invented here.
  const [terminalError, setTerminalError] = useState<{
    providerId: string;
    text: string;
    detail: string | null;
  } | null>(null);
  const terminalErrorDismissRef = useRef<HTMLButtonElement | null>(null);
  // The handoff notes live in the module store (the surface remounts on
  // navigation); this epoch only re-renders when one is recorded, cleared,
  // or dismissed.
  const [, setRunsEpoch] = useState(0);
  // Cleared after the close commits (not at the end of confirm): a second
  // synchronous click still sees the stale non-null consent, so the ref must
  // stay armed until that re-render.
  const consentInFlightRef = useRef(false);
  const consentRestoreRef = useRef<HTMLButtonElement | null>(null);
  // Panel-owned model-count cache, cleared on Refresh so counts revalidate
  // with the catalog. State-lazy, never reassigned: the identity is stable
  // across renders, so rows can safely depend on it. The epoch remounts a
  // mounted count on invalidation, so an open row re-reads without the
  // human collapsing it first.
  const [modelCache] = useState<ModelCountCache>(() => new Map());
  const [modelEpoch, setModelEpoch] = useState(0);

  function invalidateModelCounts() {
    modelCache.clear();
    setModelEpoch((epoch) => epoch + 1);
  }

  // Set on Confirm: the effect below moves focus onto the row showing the
  // npm run. Found by attribute at focus time — no node registry, no
  // render-phase ref access.
  const pendingFocusRowRef = useRef<string | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);

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
  }, [npmRun, catalog]);

  useEffect(() => {
    if (terminalError !== null) terminalErrorDismissRef.current?.focus();
  }, [terminalError]);

  // Flip a handoff note to "nothing was typed" once its take bound passes.
  // Runs on every render by design: the timer only matters while this panel
  // is mounted with a fresh, still-waiting run, and any render re-arms it.
  useEffect(() => {
    const remaining = terminalRuns()
      .filter((run) => run.typed && hasTerminalInput(run.sessionId))
      .map((run) => run.atMs + TERMINAL_TAKE_TIMEOUT_MS - Date.now())
      .filter((ms) => ms > 0);
    if (remaining.length === 0) return;
    const timer = setTimeout(() => setRunsEpoch((epoch) => epoch + 1), Math.min(...remaining));
    return () => clearTimeout(timer);
  });

  useEffect(() => {
    consentInFlightRef.current = false;
    if (consent === null) {
      consentRestoreRef.current?.focus();
      consentRestoreRef.current = null;
    }
  }, [consent]);

  // The handshake's own capability list, through the same channel every other
  // surface reads it: the supervisor's `daemon_status`.
  const daemon = useSettingsDaemon();
  const selectSurface = useAppStore((state) => state.selectSurface);
  // The workspace a terminal handoff opens under: the "+" menu's own rule
  // is that a terminal without one starts in the daemon's directory, so
  // without one this page offers no tab at all (headless install instead).
  const terminalWorkspaceId = getLastSelectedWorkspaceId();
  const toolPolicySupported = daemon.capabilities.includes(TOOL_POLICY_CAPABILITY);
  const providerSwitchSupported = daemon.capabilities.includes(PROVIDER_SWITCHES_CAPABILITY);
  const providerAuthCheckSupported = daemon.capabilities.includes(PROVIDER_AUTH_CHECK_CAPABILITY);
  const vocabularySupported = daemon.capabilities.includes(PROVIDER_VOCABULARY_CAPABILITY);

  const providers = useMemo(() => catalog?.providers ?? null, [catalog]);
  const installed = useMemo(
    () => (providers ?? []).filter((provider) => provider.installed !== false),
    [providers],
  );
  // Registry agents run on demand through npx (the old "available via npx"):
  // no local executable, so they get their own group and must not crowd
  // the real CLIs under "Installed".
  const localProviders = useMemo(
    () => installed.filter((provider) => provider.origin !== "npx-wrapper"),
    [installed],
  );
  const npxProviders = useMemo(
    () => installed.filter((provider) => provider.origin === "npx-wrapper"),
    [installed],
  );
  const available = useMemo(
    () => (providers ?? []).filter((provider) => provider.installed === false),
    [providers],
  );
  const trimmedQuery = query.trim().toLowerCase();
  const visibleAvailable = useMemo(
    () =>
      trimmedQuery === ""
        ? available
        : available.filter((provider) => provider.id.toLowerCase().includes(trimmedQuery)),
    [available, trimmedQuery],
  );
  const toolProviderCount = useMemo(
    () => installed.filter((provider) => (provider.tools ?? []).length > 0).length,
    [installed],
  );
  const toolStore = useToolPolicies(toolPolicySupported, toolProviderCount > 0);
  // Failed-closed policy: the daemon denies every restrictable tool while
  // `toolPolicyError` is set. Missing rows render as denied (never as
  // allowed) and the toggles lock — a write would be refused anyway.
  const toolPolicyFailedClosed = daemon.toolPolicyError != null;
  const providerSwitches = useProviderSwitches(providerSwitchSupported);
  const { beginFetch, reconcile } = providerSwitches;

  useEffect(() => {
    let cancelled = false;
    const seq = ++fetchSeqRef.current;
    const switchFetch = beginFetch();
    void providersList()
      .then(async (listed) => {
        if (cancelled || seq !== fetchSeqRef.current) return;
        reconcile(listed.providers, switchFetch);
        setCatalog(listed);
      })
      .catch((cause: unknown) => {
        if (!cancelled && seq === fetchSeqRef.current) {
          setCatalog({ providers: [], unreadableDirs: 0 });
          setError(errorSentence(cause));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [beginFetch, reconcile]);

  // The check runs on open and on explicit Refresh only. A mount (or a
  // capability change) bumps this epoch; an install or update never does,
  // so a completed install cannot spawn a silent extra round. The epoch is
  // a boolean dep, not the capabilities array: the 2 s status poll mints a
  // new array identity every tick, and deping on it would spawn a round
  // every two seconds.
  // This effect must run before the check effect below: React runs
  // effects in declaration order, and a check effect that reads the
  // pre-bump epoch fires a second round when the bump re-runs it.
  const authCheckEpochRef = useRef(0);
  useEffect(() => {
    if (providerAuthCheckSupported) authCheckEpochRef.current += 1;
  }, [providerAuthCheckSupported]);

  useEffect(() => {
    if (!providerAuthCheckSupported || catalog === null) return;
    const epoch = authCheckEpochRef.current;
    if (authCheckDoneEpochRef.current === epoch) return;
    // Set eagerly: a check that rejects is never retried for this mount —
    // the row keeps last-start wording until a Refresh or a remount.
    authCheckDoneEpochRef.current = epoch;
    // The fetch sequence at fire time: a result for an older request never
    // overwrites a newer catalog. The daemon's coalescing and reuse window
    // make its observations monotonic per provider, but the client keeps
    // its own guard rather than rely on that.
    const seq = fetchSeqRef.current;
    let cancelled = false;
    void providersAuthCheck(false)
      .then((checked) => {
        if (!cancelled && authCheckEpochRef.current === epoch && seq === fetchSeqRef.current) {
          setCatalog((current) => (current === null ? current : mergeAuthChecks(current, checked)));
        }
      })
      .catch(() => {
        // Catalog discovery still works when an auth check cannot run.
      });
    return () => {
      cancelled = true;
    };
  }, [catalog, providerAuthCheckSupported]);

  function refresh() {
    if (refreshInFlightRef.current) return;
    refreshInFlightRef.current = true;
    setRefreshing(true);
    setError(null);
    setTerminalError(null);
    // Counts belong to the old catalog: drop them so the next expand re-reads.
    invalidateModelCounts();
    const seq = ++fetchSeqRef.current;
    const switchFetch = beginFetch();
    void providersRefresh()
      .then(async (fresh) => {
        let checkedCatalog = fresh;
        try {
          if (providerAuthCheckSupported) {
            // A deliberate Refresh measures again: it must never be served
            // the reuse window's possibly-stale observation.
            checkedCatalog = mergeAuthChecks(fresh, await providersAuthCheck(true));
          }
        } catch {
          // Keep the fresh catalog and its last-start fallback.
        }
        if (seq === fetchSeqRef.current) {
          reconcile(checkedCatalog.providers, switchFetch);
          setCatalog(checkedCatalog);
        }
        // The refetch is the proof a handoff landed: installed rows move
        // sections, so waiting notes clear only on success — a Refresh
        // mid-install must not re-offer Install for a run still going.
        clearTerminalRuns();
        setRunsEpoch((epoch) => epoch + 1);
      })
      .catch((cause: unknown) => {
        if (seq === fetchSeqRef.current) {
          setError(errorSentence(cause));
        }
      })
      .finally(() => {
        // Unconditional on purpose: React 18+ treats setState on an unmounted
        // component as a safe no-op, so the button can never get stuck on
        // "Refreshing…". Do not re-add an unmount guard here.
        refreshInFlightRef.current = false;
        setRefreshing(false);
      });
  }

  // A terminal tab the person can paste into needs no shell answer; an
  // install typed for them does. Fetch only for that case.
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
      // StrictMode (see refresh() above).
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
        setRunsEpoch((epoch) => epoch + 1);
        setTerminalError(null);
        // create() already selected the tab; the surface switch remounts
        // this panel, so the handoff note lives in the module store. And
        // only while this panel is still the surface: a slow create must
        // not yank the person back from where they went meanwhile.
        if (useAppStore.getState().activeSurface === "settings") selectSurface("workspace");
      });
  }

  /** Both shell variants for the paste note, labeled. Null when no plan
   * exists for either — unreachable (the Install button needs a package),
   * but the note must never show half a fallback. */
  function copyPlanLines(provider: ProviderInfo): CopyableLine[] | null {
    const powershell = providerInstallPlan(provider, "powershell");
    const posix = providerInstallPlan(provider, "posix");
    const powershellLine = powershell?.lines[0];
    const posixLine = posix?.lines[0];
    if (powershellLine === undefined || posixLine === undefined) return null;
    return [
      { label: SHELL_LABELS.powershell, text: powershellLine },
      { label: SHELL_LABELS.posix, text: posixLine },
    ];
  }

  /** The install note behind a terminal or copy consent: the login note
   * when the provider documents one, else the no-login explanation. */
  function installNote(provider: ProviderInfo): string | null {
    return providerLoginPlan(provider)?.note ?? providerNoLoginNote(provider.id);
  }

  const unreadableDirs = catalog?.unreadableDirs ?? 0;
  // The headless road's one line (update always; install when no workspace
  // can host a tab — the daemon allowlists the package by id, so even a
  // line our own pattern refused stays safe here).
  function headlessLine(provider: ProviderInfo): string | null {
    return provider.npmPackage ? `npm install -g ${provider.npmPackage}@latest` : null;
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
      // Update keeps its global-npm warning verbatim.
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
  /**
   * What a recorded run says on its row: the handoff while fresh and
   * taken, the paste fallback when the tab opened untyped or never picked
   * the lines up past the take bound. Never "installing": nothing here
   * observes the install.
   */
  function runNotice(run: ProviderTerminalRun): { text: string; lines: CopyableLine[] } {
    const display = terminalRunDisplay(run);
    if (display === "paste")
      return { text: "Terminal tab opened — paste the copied line there.", lines: run.lines };
    if (display === "expired")
      return {
        text: "Nothing was typed in the terminal tab — copy the line and paste it there.",
        lines: run.lines,
      };
    return { text: terminalRunNotice(run.verb), lines: [] };
  }

  function renderInstalledRow(provider: ProviderInfo, viaNpx: boolean) {
    const withTools = toolPolicySupported && (provider.tools ?? []).length > 0;
    const runHere = npmRun?.providerId === provider.id;
    const runHereNotice = terminalRuns().find((run) => run.providerId === provider.id) ?? null;
    // Log in needs a tab under a workspace; without a known workspace the
    // details say so, and without a documented login they name where the
    // login lives instead. Both are hints, never buttons to nowhere.
    const loginDocumented = providerLogin(provider.id) !== null;
    const canTerminalHere = terminalWorkspaceId !== null;
    return (
      <ProviderRow
        key={provider.id}
        provider={provider}
        enabled={providerSwitches.isEnabled(provider)}
        onToggleProvider={(next) => void providerSwitches.setEnabled(provider, next)}
        providerWriteError={providerSwitches.states[provider.id]?.error?.sentence ?? null}
        providerSwitchSupported={providerSwitchSupported}
        toolPolicy={
          withTools ? toolPolicyFor(provider.id, toolStore.policies, toolPolicyFailedClosed) : null
        }
        toolsDisabled={toolStore.policies === null || toolPolicyFailedClosed}
        vocabularySupported={vocabularySupported}
        modelCache={modelCache}
        consent={consent?.provider.id === provider.id ? consentView(provider, consent.verb) : null}
        npmFailure={npmFailure?.providerId === provider.id ? npmFailure : null}
        terminalNotice={runHereNotice !== null ? runNotice(runHereNotice) : null}
        onDismissNotice={() => {
          clearTerminalRun(provider.id);
          setRunsEpoch((epoch) => epoch + 1);
        }}
        writeError={toolStore.writeErrors[provider.id] ?? null}
        onDismissWriteError={() => toolStore.dismissWriteError(provider.id)}
        busyVerb={runHere && npmRun ? npmRun.verb : null}
        actionsDisabled={npmRun !== null}
        modelEpoch={modelEpoch}
        viaNpx={viaNpx}
        onToggleTools={(next) => toolStore.setEnabled(provider.id, next)}
        onTurnAllOn={() => toolStore.turnAllOn(provider.id)}
        onOpenUpdate={(trigger) => openConsent(provider, "update", trigger)}
        onOpenLogin={
          loginDocumented && canTerminalHere
            ? (trigger) => openConsent(provider, "login", trigger)
            : undefined
        }
        loginHint={
          loginDocumented
            ? canTerminalHere
              ? null
              : "Log in needs an open workspace."
            : (providerNoLoginNote(provider.id) ?? null)
        }
        onConfirmConsent={confirmConsent}
        onCancelConsent={() => setConsent(null)}
        onDismissFailure={() => setNpmFailure(null)}
        onRefresh={refresh}
      />
    );
  }
  return (
    <div id="settings-panel-providers" ref={panelRef}>
      <button className="provider-refresh" type="button" disabled={refreshing} onClick={refresh}>
        {refreshing ? "Refreshing…" : "Refresh"}
      </button>
      {toolPolicyFailedClosed && daemon.toolPolicyError ? (
        <ToolPolicyBanner reason={daemon.toolPolicyError} />
      ) : null}
      {error ? (
        <div role="alert">
          <ErrorText
            sentence={error.sentence}
            detail={error.detail}
            id="settings-providers-error"
          />
        </div>
      ) : null}
      {terminalError ? (
        <div role="alert" className="provider-card-block provider-update-error">
          <span
            title={terminalError.detail ?? undefined}
            aria-describedby={terminalError.detail ? "settings-terminal-error-detail" : undefined}
          >
            {terminalError.providerId}: {terminalError.text}
            {terminalError.detail ? (
              <span id="settings-terminal-error-detail" className="error-detail-sr-only">
                {terminalError.detail}
              </span>
            ) : null}
          </span>
          <button
            ref={terminalErrorDismissRef}
            type="button"
            className="provider-refresh provider-update-error-dismiss"
            onClick={() => setTerminalError(null)}
          >
            Dismiss
          </button>
        </div>
      ) : null}
      {providers === null ? (
        <div role="status">Looking for agent CLIs on PATH…</div>
      ) : providers.length === 0 ? (
        <div className="provider-empty" role="status">
          <div>
            {unreadableDirs > 0
              ? `No agent CLI found, but ${unreadableDirs} PATH directories could not be read`
              : "No agent CLI found on PATH"}
          </div>
          <p>
            Install an agent CLI such as grok, claude, or gemini and restart Devboule. Until then
            there is no provider to start a session with.
          </p>
        </div>
      ) : (
        <>
          {toolStore.loadError ? (
            <div role="alert" className="prov-tools-error">
              <ErrorText
                sentence={toolStore.loadError.sentence}
                detail={toolStore.loadError.detail}
                id="settings-tool-policy-error"
              />
              <button type="button" className="settings-device-action" onClick={toolStore.retry}>
                Retry
              </button>
            </div>
          ) : null}
          {localProviders.length > 0 ? (
            <section aria-label="Installed">
              <h3 className="settings-subheading">Installed</h3>
              <div className="prov-card" aria-busy={refreshing || npmRun !== null}>
                {localProviders.map((provider) => renderInstalledRow(provider, false))}
              </div>
            </section>
          ) : null}
          {available.length > 0 ? (
            <section aria-label="Available to install">
              <h3 className="settings-subheading">Available to install</h3>
              <input
                type="search"
                className="prov-search"
                placeholder="Search the catalogue"
                aria-label="Search available providers"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
              />
              {visibleAvailable.length === 0 ? (
                <p className="prov-no-match" role="status">
                  No providers match this search.
                </p>
              ) : (
                <div className="prov-card" aria-busy={refreshing}>
                  {visibleAvailable.map((provider) => {
                    const runHere = npmRun?.providerId === provider.id;
                    const consentHere = consent?.provider.id === provider.id;
                    const availableNotice =
                      terminalRuns().find((run) => run.providerId === provider.id) ?? null;
                    // Install is offered only when a safe line exists: the
                    // package check refuses names outside the strict shape.
                    // The shell it will be typed for is resolved at open.
                    const installable = providerInstallPackage(provider) !== null;
                    const noticeView = availableNotice !== null ? runNotice(availableNotice) : null;
                    const consentHereView =
                      consentHere && consent !== null ? consentView(provider, consent.verb) : null;
                    return (
                      <div
                        className="prov-available-row"
                        key={provider.id}
                        tabIndex={-1}
                        data-provider-row={provider.id}
                      >
                        <span className="prov-available-main">
                          <span className="prov-name">{provider.id}</span>
                          {(provider.npmPackage ?? provider.executable) ? (
                            <span className="prov-available-sub">
                              {provider.npmPackage ?? provider.executable}
                            </span>
                          ) : null}
                          <ProviderVersionLine provider={provider} />
                        </span>
                        {noticeView !== null ? (
                          <span className="provider-card-block prov-terminal-note" role="status">
                            {noticeView.text}
                            {noticeView.lines.length > 0 ? (
                              <CopyableLines lines={noticeView.lines} />
                            ) : null}
                            <button
                              type="button"
                              className="provider-refresh provider-update-error-dismiss"
                              onClick={() => {
                                clearTerminalRun(provider.id);
                                setRunsEpoch((epoch) => epoch + 1);
                              }}
                            >
                              Dismiss
                            </button>
                          </span>
                        ) : runHere && npmRun ? (
                          <span className="prov-busy" role="status">
                            {npmRun.verb === "install" ? "Installing…" : "Updating…"}
                          </span>
                        ) : installable ? (
                          <button
                            className="provider-refresh provider-install"
                            type="button"
                            disabled={npmRun !== null}
                            onClick={(event) =>
                              openConsent(provider, "install", event.currentTarget)
                            }
                          >
                            Install
                          </button>
                        ) : null}
                        {consentHereView === "waiting" ? (
                          <div
                            className="provider-card-block provider-consent"
                            role="group"
                            aria-label={`Confirm install for ${provider.id}`}
                          >
                            <p className="provider-consent-notice">{SHELL_QUERY_LOADING}</p>
                            <div className="provider-consent-actions">
                              <button
                                type="button"
                                className="provider-refresh provider-consent-cancel"
                                onClick={() => setConsent(null)}
                              >
                                Cancel
                              </button>
                            </div>
                          </div>
                        ) : consentHereView !== null ? (
                          <ProviderConsentBlock
                            providerId={provider.id}
                            verb={consentHereView.verb}
                            lines={consentHereView.lines}
                            copyLines={consentHereView.copyLines}
                            notice={consentHereView.notice}
                            onConfirm={confirmConsent}
                            onCancel={() => setConsent(null)}
                          />
                        ) : null}
                        {npmFailure?.providerId === provider.id ? (
                          <ProviderNpmFailure
                            text={npmFailure.text}
                            detail={npmFailure.detail}
                            onDismiss={() => setNpmFailure(null)}
                          />
                        ) : null}
                      </div>
                    );
                  })}
                </div>
              )}
            </section>
          ) : null}
          {npxProviders.length > 0 ? (
            <section aria-label="Run on demand (npx)">
              <h3 className="settings-subheading">Run on demand (npx)</h3>
              <p className="prov-group-note">
                These providers start on demand through npx — nothing is installed for them on this
                machine.
              </p>
              <div className="prov-card" aria-busy={refreshing || npmRun !== null}>
                {npxProviders.map((provider) => renderInstalledRow(provider, true))}
              </div>
            </section>
          ) : null}
          {unreadableDirs > 0 ? (
            <p className="provider-empty" role="status">
              {unreadableDirs} PATH directories could not be read
            </p>
          ) : null}
        </>
      )}
    </div>
  );
}
