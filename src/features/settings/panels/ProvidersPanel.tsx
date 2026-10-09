import { useEffect, useMemo, useRef, useState } from "react";
import { ErrorText } from "../../../components/ErrorText";
import { providerEmptySentence } from "../../../lib/providerEmptySentence";
import { isInstalled, isRunOnDemand } from "../../../lib/providerPredicates";
import { hasTerminalInput } from "../../terminal/pendingTerminalInput";
import { useSettingsDaemon } from "../settingsDaemon";
import {
  PROVIDER_AUTH_CHECK_CAPABILITY,
  PROVIDER_SWITCHES_CAPABILITY,
  TOOL_POLICY_CAPABILITY,
  toolPolicyFor,
} from "../providerStatus";
import type { ProviderInfo } from "../../../types/ipc";
import {
  PROVIDER_VOCABULARY_CAPABILITY,
  type ModelCountCache,
} from "../providers/ProviderModelCount";
import {
  TERMINAL_TAKE_TIMEOUT_MS,
  clearTerminalRun,
  clearTerminalRuns,
  terminalRuns,
} from "../providers/providerTerminalRuns";
import { providerLogin, providerNoLoginNote } from "../providers/providerTerminalCommands";
import { ProviderRow } from "../providers/ProviderRow";
import { ProviderAvailableRow } from "../providers/ProviderAvailableRow";
import { ProvidersTerminalError } from "../providers/ProvidersTerminalError";
import { runNotice } from "../providers/providerPanelCopy";
import { useToolPolicies } from "../providers/useToolPolicies";
import { ToolPolicyBanner } from "../providers/ToolPolicyBanner";
import { useProviderSwitches } from "../providers/useProviderSwitches";
import { useProviderCatalog } from "../providers/useProviderCatalog";
import { useProviderConsent } from "../providers/useProviderConsent";
import "../providers.css";

/**
 * The Providers page: an Installed section (one row per CLI: status, the
 * enable switch, the row menu), an Available to install section with catalogue
 * search and Install buttons, and a Run on demand section for npx providers.
 * Install, update and login share one consent block and one npm-failure line;
 * the model count behind each Ready status is lazy (see `ProviderModelCount`).
 */
export function ProvidersPanel() {
  const [query, setQuery] = useState("");
  // Bumped whenever a handoff note is recorded, cleared, or dismissed.
  const [, setRunsEpoch] = useState(0);
  const bumpRuns = () => setRunsEpoch((epoch) => epoch + 1);
  // Panel-owned model-count cache, cleared on Refresh so counts revalidate
  // with the catalog. State-lazy, never reassigned: the identity is stable
  // across renders, so rows can safely depend on it. The epoch remounts a
  // mounted count on invalidation, so an open row re-reads without the
  // human collapsing it first.
  const [modelCache] = useState<ModelCountCache>(() => new Map());
  const [modelEpoch, setModelEpoch] = useState(0);
  const panelRef = useRef<HTMLDivElement>(null);

  function invalidateModelCounts() {
    modelCache.clear();
    setModelEpoch((epoch) => epoch + 1);
  }

  // The handshake's own capability list, through the same channel every other
  // surface reads it: the supervisor's `daemon_status`.
  const daemon = useSettingsDaemon();
  const toolPolicySupported = daemon.capabilities.includes(TOOL_POLICY_CAPABILITY);
  const providerSwitchSupported = daemon.capabilities.includes(PROVIDER_SWITCHES_CAPABILITY);
  const providerAuthCheckSupported = daemon.capabilities.includes(PROVIDER_AUTH_CHECK_CAPABILITY);
  const vocabularySupported = daemon.capabilities.includes(PROVIDER_VOCABULARY_CAPABILITY);

  const providerSwitches = useProviderSwitches(providerSwitchSupported);
  const catalogState = useProviderCatalog({
    authCheckSupported: providerAuthCheckSupported,
    switches: providerSwitches,
    onRefreshStart: () => {
      consent.dismissTerminalError();
      invalidateModelCounts();
    },
    // The refetch is the proof a handoff landed: waiting notes clear only on success.
    onRefreshed: () => {
      clearTerminalRuns();
      bumpRuns();
    },
  });
  const { catalog, error, refreshing, refresh } = catalogState;
  const consent = useProviderConsent({
    catalog,
    panelRef,
    switches: providerSwitches,
    setCatalog: catalogState.setCatalog,
    setError: catalogState.setError,
    fetchSeqRef: catalogState.fetchSeqRef,
    invalidateModelCounts,
    onRunsChanged: bumpRuns,
  });
  const { npmRun, npmFailure, terminalError, terminalWorkspaceId } = consent;

  const providers = useMemo(() => catalog?.providers ?? null, [catalog]);
  const installed = useMemo(() => (providers ?? []).filter(isInstalled), [providers]);
  // Registry agents run on demand through npx (the old "available via npx"):
  // no local executable, so they get their own group and must not crowd
  // the real CLIs under "Installed".
  const localProviders = useMemo(
    () => installed.filter((provider) => !isRunOnDemand(provider)),
    [installed],
  );
  const npxProviders = useMemo(() => installed.filter(isRunOnDemand), [installed]);
  const available = useMemo(
    () => (providers ?? []).filter((provider) => !isInstalled(provider)),
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

  // Flip a handoff note to "nothing was typed" once its take bound passes.
  // Runs on every render by design: the timer only matters while this panel
  // is mounted with a fresh, still-waiting run, and any render re-arms it.
  useEffect(() => {
    const remaining = terminalRuns()
      .filter((run) => run.typed && hasTerminalInput(run.sessionId))
      .map((run) => run.atMs + TERMINAL_TAKE_TIMEOUT_MS - Date.now())
      .filter((ms) => ms > 0);
    if (remaining.length === 0) return;
    const timer = setTimeout(() => bumpRuns(), Math.min(...remaining));
    return () => clearTimeout(timer);
  });

  const unreadableDirs = catalog?.unreadableDirs ?? 0;

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
        consent={
          consent.consent?.provider.id === provider.id
            ? consent.consentView(provider, consent.consent.verb)
            : null
        }
        npmFailure={npmFailure?.providerId === provider.id ? npmFailure : null}
        terminalNotice={runHereNotice !== null ? runNotice(runHereNotice) : null}
        onDismissNotice={() => {
          clearTerminalRun(provider.id);
          bumpRuns();
        }}
        writeError={toolStore.writeErrors[provider.id] ?? null}
        onDismissWriteError={() => toolStore.dismissWriteError(provider.id)}
        busyVerb={runHere && npmRun ? npmRun.verb : null}
        actionsDisabled={npmRun !== null}
        modelEpoch={modelEpoch}
        viaNpx={viaNpx}
        onToggleTools={(next) => toolStore.setEnabled(provider.id, next)}
        onTurnAllOn={() => toolStore.turnAllOn(provider.id)}
        onOpenUpdate={(trigger) => consent.openConsent(provider, "update", trigger)}
        onOpenLogin={
          loginDocumented && canTerminalHere
            ? (trigger) => consent.openConsent(provider, "login", trigger)
            : undefined
        }
        loginHint={
          loginDocumented
            ? canTerminalHere
              ? null
              : "Log in needs an open workspace."
            : (providerNoLoginNote(provider.id) ?? null)
        }
        onConfirmConsent={consent.confirmConsent}
        onCancelConsent={consent.closeConsent}
        onDismissFailure={consent.dismissFailure}
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
        <ProvidersTerminalError error={terminalError} onDismiss={consent.dismissTerminalError} />
      ) : null}
      {providers === null ? (
        <div role="status">Looking for agent CLIs on PATH…</div>
      ) : providers.length === 0 ? (
        <div className="provider-empty" role="status">
          <div>{providerEmptySentence(unreadableDirs)}</div>
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
                    const consentHere = consent.consent?.provider.id === provider.id;
                    const availableNotice =
                      terminalRuns().find((run) => run.providerId === provider.id) ?? null;
                    return (
                      <ProviderAvailableRow
                        key={provider.id}
                        provider={provider}
                        busyVerb={runHere && npmRun ? npmRun.verb : null}
                        actionsDisabled={npmRun !== null}
                        notice={availableNotice !== null ? runNotice(availableNotice) : null}
                        consent={
                          consentHere && consent.consent !== null
                            ? consent.consentView(provider, consent.consent.verb)
                            : null
                        }
                        npmFailure={npmFailure?.providerId === provider.id ? npmFailure : null}
                        onInstall={(trigger) => consent.openConsent(provider, "install", trigger)}
                        onDismissNotice={() => {
                          clearTerminalRun(provider.id);
                          bumpRuns();
                        }}
                        onConfirm={consent.confirmConsent}
                        onCancel={consent.closeConsent}
                        onDismissFailure={consent.dismissFailure}
                      />
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
