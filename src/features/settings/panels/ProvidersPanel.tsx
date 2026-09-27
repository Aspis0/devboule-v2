import { useEffect, useMemo, useRef, useState } from "react";
import { providerUpdate, providersList, providersRefresh } from "../../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import { ErrorText } from "../../../components/ErrorText";
import { useSettingsDaemon } from "../settingsDaemon";
import { TOOL_POLICY_CAPABILITY, logTail, toolPolicyFor } from "../providerStatus";
import type { ProviderCatalog, ProviderInfo } from "../../../types/ipc";
import {
  PROVIDER_VOCABULARY_CAPABILITY,
  type ModelCountCache,
} from "../providers/ProviderModelCount";
import { ProviderConsentBlock } from "../providers/ProviderConsentBlock";
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
  verb: "update" | "install";
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
  // Cleared after the close commits (not at the end of confirm): a second
  // synchronous click still sees the stale non-null consent, so the ref must
  // stay armed until that re-render.
  const consentInFlightRef = useRef(false);
  const consentRestoreRef = useRef<HTMLButtonElement | null>(null);
  // Panel-owned model-count cache, cleared on Refresh so counts revalidate
  // with the catalog. State-lazy, never reassigned: the identity is stable
  // across renders, so rows can safely depend on it.
  const [modelCache] = useState<ModelCountCache>(() => new Map());

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
  const toolPolicySupported = daemon.capabilities.includes(TOOL_POLICY_CAPABILITY);
  const vocabularySupported = daemon.capabilities.includes(PROVIDER_VOCABULARY_CAPABILITY);

  const providers = useMemo(() => catalog?.providers ?? null, [catalog]);
  const installed = useMemo(
    () => (providers ?? []).filter((provider) => provider.installed !== false),
    [providers],
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
    // Counts belong to the old catalog: drop them so the next expand re-reads.
    modelCache.clear();
    const seq = ++fetchSeqRef.current;
    void providersRefresh()
      .then((fresh) => {
        if (seq === fetchSeqRef.current) setCatalog(fresh);
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
    verb: "update" | "install",
    trigger: HTMLButtonElement | null,
  ) {
    if (npmRun !== null) return;
    consentRestoreRef.current = trigger;
    setConsent({ provider, verb });
  }

  function confirmConsent() {
    if (consent === null || consentInFlightRef.current) return;
    consentInFlightRef.current = true;
    const { provider } = consent;
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

  const unreadableDirs = catalog?.unreadableDirs ?? 0;
  const npmCommand =
    consent !== null && consent.provider.npmPackage
      ? `npm install -g ${consent.provider.npmPackage}@latest`
      : null;
  return (
    <div id="settings-panel-providers">
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
          {toolStore.loadFailed && toolStore.loadError ? (
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
          {installed.length > 0 ? (
            <section aria-label="Installed">
              <h3 className="prov-section-label">Installed</h3>
              <div className="prov-card" aria-busy={refreshing || npmRun !== null}>
                {installed.map((provider) => {
                  const withTools = toolPolicySupported && (provider.tools ?? []).length > 0;
                  const runHere = npmRun?.providerId === provider.id;
                  return (
                    <ProviderRow
                      key={provider.id}
                      provider={provider}
                      toolPolicy={withTools ? toolPolicyFor(provider.id, toolStore.policies) : null}
                      toolsDisabled={toolStore.policies === null}
                      vocabularySupported={vocabularySupported}
                      modelCache={modelCache}
                      consentOpen={consent?.provider.id === provider.id}
                      npmCommand={consent?.provider.id === provider.id ? npmCommand : null}
                      npmVerb={consent?.provider.id === provider.id ? consent.verb : null}
                      npmFailure={npmFailure?.providerId === provider.id ? npmFailure : null}
                      writeError={
                        toolStore.writeError?.providerId === provider.id
                          ? toolStore.writeError.error
                          : null
                      }
                      busyVerb={runHere && npmRun ? npmRun.verb : null}
                      actionsDisabled={npmRun !== null}
                      onToggleTools={(next) => toolStore.setEnabled(provider.id, next)}
                      onTurnAllOn={() => toolStore.turnAllOn(provider.id)}
                      onOpenUpdate={(trigger) => openConsent(provider, "update", trigger)}
                      onConfirmConsent={confirmConsent}
                      onCancelConsent={() => setConsent(null)}
                      onDismissFailure={() => setNpmFailure(null)}
                      onRefresh={refresh}
                    />
                  );
                })}
              </div>
            </section>
          ) : null}
          {available.length > 0 ? (
            <section aria-label="Available to install">
              <h3 className="prov-section-label">Available to install</h3>
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
                <div className="prov-card" aria-busy={refreshing || npmRun !== null}>
                  {visibleAvailable.map((provider) => {
                    const runHere = npmRun?.providerId === provider.id;
                    const consentHere = consent?.provider.id === provider.id;
                    return (
                      <div className="prov-available-row" key={provider.id}>
                        <span className="prov-available-main">
                          <span className="prov-name">{provider.id}</span>
                          <span className="prov-available-sub">
                            {provider.npmPackage ?? provider.executable}
                          </span>
                          <ProviderVersionLine provider={provider} />
                        </span>
                        {runHere && npmRun ? (
                          <span className="prov-busy" role="status">
                            {npmRun.verb === "install" ? "Installing…" : "Updating…"}
                          </span>
                        ) : provider.npmPackage ? (
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
                        {consentHere && npmCommand !== null && consent !== null ? (
                          <ProviderConsentBlock
                            providerId={provider.id}
                            verb={consent.verb}
                            command={npmCommand}
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
