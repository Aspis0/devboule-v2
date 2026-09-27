import { useEffect, useMemo, useRef, useState } from "react";
import { providerUpdate, providersList, providersRefresh } from "../../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import { ErrorText } from "../../../components/ErrorText";
import { useAppStore } from "../../../store/appStore";
import { sharedSessionController } from "../../workspace/workspaceSessions";
import { requestTerminalInput } from "../../terminal/pendingTerminalInput";
import { useSettingsDaemon } from "../settingsDaemon";
import { TOOL_POLICY_CAPABILITY, logTail, toolPolicyFor } from "../providerStatus";
import { ProviderConsentBlock } from "../providers/ProviderConsentBlock";
import type { ProviderCatalog, ProviderInfo } from "../../../types/ipc";
import {
  PROVIDER_VOCABULARY_CAPABILITY,
  type ModelCountCache,
} from "../providers/ProviderModelCount";
import {
  detectTerminalShell,
  providerLogin,
  providerNoLoginNote,
  providerTerminalPlan,
} from "../providers/providerTerminalCommands";
import { getLastSelectedWorkspaceId } from "../../workspace/lastSelectedWorkspace";
import {
  clearTerminalRun,
  clearTerminalRuns,
  recordTerminalRun,
  terminalRuns,
} from "../providers/providerTerminalRuns";
import { ProviderNpmFailure } from "../providers/ProviderNpmFailure";
import { ProviderRow, ProviderVersionLine } from "../providers/ProviderRow";
import { useToolPolicies } from "../providers/useToolPolicies";
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

/** The consent's own words: what Confirm types, and what it changes. */
const NPM_WARNING =
  "This changes your global npm installation; running sessions keep the old version until they are restarted.";
const TERMINAL_LEAD = "Confirm opens a terminal tab and types this line, then takes you there.";
// The PTY starts `-NoProfile`, so a profile-provided npm is absent here —
// the daemon road's one honest sentence, restored where it can now happen.
const NPM_MISSING =
  "If the tab says npm is not recognized, install Node.js/npm and try again — the tab starts without your shell profile.";
const NO_WORKSPACE_INSTALL =
  "No workspace is open, so this installs in the background instead of a terminal tab.";

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
  // A terminal tab the daemon would not start: panel-level, because the
  // row that asked may belong to either section and the shared roster owns
  // the daemon's own words (Settings cannot see them).
  const [terminalError, setTerminalError] = useState<{
    providerId: string;
    text: string;
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
  const terminalShell = detectTerminalShell();
  const toolPolicySupported = daemon.capabilities.includes(TOOL_POLICY_CAPABILITY);
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

  useEffect(() => {
    let cancelled = false;
    const seq = ++fetchSeqRef.current;
    void providersList()
      .then((listed) => {
        if (!cancelled && seq === fetchSeqRef.current) setCatalog(listed);
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
  }, []);

  function refresh() {
    if (refreshInFlightRef.current) return;
    refreshInFlightRef.current = true;
    setRefreshing(true);
    setError(null);
    setTerminalError(null);
    // Counts belong to the old catalog: drop them so the next expand re-reads.
    invalidateModelCounts();
    const seq = ++fetchSeqRef.current;
    void providersRefresh()
      .then((fresh) => {
        if (seq === fetchSeqRef.current) setCatalog(fresh);
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

  function openConsent(
    provider: ProviderInfo,
    verb: "update" | "install" | "login",
    trigger: HTMLButtonElement | null,
  ) {
    if (npmRun !== null) return;
    consentRestoreRef.current = trigger;
    setConsent({ provider, verb });
  }

  function confirmConsent() {
    if (consent === null || consentInFlightRef.current) return;
    consentInFlightRef.current = true;
    // Terminal handoff when a workspace can host the tab; headless daemon
    // npm otherwise (update always, install when no workspace is open —
    // the daemon needs no cwd). A login without a workspace has no entry
    // points, so reaching here with one is a dead branch, never a send.
    if (consent.verb !== "update" && terminalWorkspaceId !== null) {
      const { provider, verb } = consent;
      setConsent(null);
      confirmTerminal(provider, verb, terminalWorkspaceId);
      return;
    }
    if (consent.verb === "login") {
      setConsent(null);
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
        void providersList()
          .then((fresh) => {
            if (seq === fetchSeqRef.current) setCatalog(fresh);
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
   * which the headless npm road cannot do.
   */
  function confirmTerminal(provider: ProviderInfo, verb: "install" | "login", workspaceId: string) {
    const plan = providerTerminalPlan(provider, verb, terminalShell);
    if (plan === null) return;
    // The controller drops a create while one is in flight without asking
    // the daemon: say that plainly, never the daemon-refusal sentence.
    if (sharedSessionController().getState().creating) {
      setTerminalError({
        providerId: provider.id,
        text: "A terminal is already starting — wait a moment and try again.",
      });
      return;
    }
    void sharedSessionController()
      .create("terminal", null, workspaceId)
      .then((session) => {
        if (session === null) {
          setTerminalError({
            providerId: provider.id,
            text: "Could not open a terminal tab. The daemon did not start one — try again.",
          });
          return;
        }
        requestTerminalInput(session.id, plan.lines);
        recordTerminalRun(provider.id, verb);
        setRunsEpoch((epoch) => epoch + 1);
        setTerminalError(null);
        // create() already selected the tab; the surface switch remounts
        // this panel, so the handoff note lives in the module store. And
        // only while this panel is still the surface: a slow create must
        // not yank the person back from where they went meanwhile.
        if (useAppStore.getState().activeSurface === "settings") selectSurface("workspace");
      });
  }

  const unreadableDirs = catalog?.unreadableDirs ?? 0;
  // The headless road's one line (update always; install when no workspace
  // can host a tab — the daemon allowlists the package by id, so even a
  // line our own pattern refused stays safe here). Terminal install and
  // login show their gated plan through consentPlan below.
  const npmCommand =
    consent !== null &&
    (consent.verb === "update" || (consent.verb === "install" && terminalWorkspaceId === null)) &&
    consent.provider.npmPackage
      ? `npm install -g ${consent.provider.npmPackage}@latest`
      : null;
  const consentPlan =
    consent !== null && consent.verb !== "update" && terminalWorkspaceId !== null
      ? providerTerminalPlan(consent.provider, consent.verb, terminalShell)
      : null;
  // Update keeps its global-npm warning verbatim; a terminal handoff names
  // the tab first, then the npm change the install line makes, then the
  // profile caveat the PTY reintroduces.
  const consentNotice =
    consent === null
      ? null
      : consent.verb === "update"
        ? NPM_WARNING
        : terminalWorkspaceId === null
          ? consent.verb === "install"
            ? `${NO_WORKSPACE_INSTALL} ${NPM_WARNING}`
            : null
          : consentPlan === null
            ? null
            : consent.verb === "install"
              ? `${TERMINAL_LEAD} ${NPM_WARNING} ${NPM_MISSING}${consentPlan.note ? ` ${consentPlan.note}` : ""}`
              : consentPlan.note
                ? `${TERMINAL_LEAD} ${consentPlan.note}`
                : TERMINAL_LEAD;
  const consentLines =
    consent === null
      ? null
      : consent.verb === "update" || (consent.verb === "install" && terminalWorkspaceId === null)
        ? npmCommand !== null
          ? [npmCommand]
          : null
        : (consentPlan?.lines ?? null);

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
        toolPolicy={withTools ? toolPolicyFor(provider.id, toolStore.policies) : null}
        toolsDisabled={toolStore.policies === null}
        vocabularySupported={vocabularySupported}
        modelCache={modelCache}
        consentOpen={consent?.provider.id === provider.id}
        consentLines={consent?.provider.id === provider.id ? consentLines : null}
        consentNotice={consent?.provider.id === provider.id ? consentNotice : null}
        consentVerb={consent?.provider.id === provider.id ? consent.verb : null}
        npmFailure={npmFailure?.providerId === provider.id ? npmFailure : null}
        terminalNotice={runHereNotice !== null ? terminalRunNotice(runHereNotice.verb) : null}
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
          <span>
            {terminalError.providerId}: {terminalError.text}
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
                    // plan refuses packages outside the strict name shape.
                    const installPlan = providerTerminalPlan(provider, "install", terminalShell);
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
                        {availableNotice !== null ? (
                          <span className="provider-card-block prov-terminal-note" role="status">
                            {terminalRunNotice(availableNotice.verb)}
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
                        ) : installPlan !== null ? (
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
                        {consentHere && consentLines !== null && consent !== null ? (
                          <ProviderConsentBlock
                            providerId={provider.id}
                            verb={consent.verb}
                            lines={consentLines}
                            notice={consentNotice}
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
