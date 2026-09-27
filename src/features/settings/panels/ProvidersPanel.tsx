import { useEffect, useRef, useState } from "react";
import {
  providerUpdate,
  providersList,
  providersRefresh,
  toolPolicyGet,
  toolPolicySet,
} from "../../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import { ErrorText } from "../../../components/ErrorText";
import { useWorkspaceDaemon } from "../../workspace/workspaceDaemon";
import { SettingsHeading } from "../SettingsSurface";
import {
  ALWAYS_ON_REASON,
  ALWAYS_ON_TOOL,
  TOOL_POLICY_CAPABILITY,
  logTail,
  providerCanUpdate,
  providerStatusText,
  providerVersionSegments,
  toolPolicyFor,
} from "../providerStatus";
import type { ProviderCatalog, ProviderInfo, ToolPolicyEntry } from "../../../types/ipc";
import "../providers.css";
/** Muted version line under the executable path; renders nothing without data. */
export function ProviderVersionLine({ provider }: { provider: ProviderInfo }) {
  const segments = providerVersionSegments(provider);
  if (segments.length === 0) return null;
  return (
    <span className="provider-version">
      {segments.map((segment, index) => (
        <span key={segment.text} title={segment.title}>
          {index > 0 ? " · " : ""}
          {segment.text}
        </span>
      ))}
    </span>
  );
}

/** A pending npm run on one provider card: what the daemon is doing right now. */
export interface ProviderNpmRun {
  providerId: string;
  verb: "update" | "install";
}

/** A provider held open in the consent panel, waiting for the user's Confirm. */
export interface ProviderConsent {
  provider: ProviderInfo;
  verb: "update" | "install";
}

/**
 * Update applies only to npm-installed CLIs whose package is known and whose
 * latest version differs from the installed one.


/**
 * Per-provider tool toggles, under one provider card. Renders nothing when
 * `provider.tools` is empty: the daemon sends the `tools` key only for
 * providers whose sessions can host the broker (ACP families plus pi and
 * Codex since the broker switch-on), and an empty list means there is
 * nothing to toggle. It renders nothing either when the handshake did not
 * negotiate [`TOOL_POLICY_CAPABILITY`], so a daemon that cannot answer
 * `tool_policy_get` is never asked — the section is absent, not broken.
 *
 * The always-on tool stays checked and disabled with its one-line reason.
 * Every other change applies optimistically and reverts on rejection; the
 * daemon's own sentence is shown verbatim inside the card.
 */
function ProviderToolSettings({
  provider,
  toolPolicySupported,
}: {
  provider: ProviderInfo;
  /** True only when the handshake advertised `tool_policy`. */
  toolPolicySupported: boolean;
}) {
  const tools = provider.tools ?? [];
  const [policies, setPolicies] = useState<readonly ToolPolicyEntry[] | null>(null);
  const [error, setError] = useState<ErrorSentence | null>(null);
  // Synchronous mirror of `policies`. It — never the render closure — is
  // what a second rapid write reads and the base its revert applies to
  // (audit findings 1, 8).
  const policiesRef = useRef<readonly ToolPolicyEntry[] | null>(null);
  // Monotonic write sequence: only the newest write owns the UI when it
  // settles, so an older rejection can never clobber a newer row.
  const seqRef = useRef(0);
  // How many writes are currently between "sent" and "settled". This card
  // deliberately lets writes overlap (see `persist`), so it is a counter,
  // and the load effect below reads it to tell "a write was in flight when
  // this fetch started" — the question the sequence number alone cannot
  // answer — apart from "a write has settled at some point".
  const writesInFlightRef = useRef(0);
  // A failed load is terminal, not a loading state: nothing will ever arrive
  // on its own, so the card shows the daemon's sentence and a Retry instead
  // of the loading lock. `loadNonce` re-runs the load effect.
  const [loadFailed, setLoadFailed] = useState(false);
  const [loadNonce, setLoadNonce] = useState(0);
  useEffect(() => {
    // No fetch when there is nothing to toggle: the daemon omits `tools`
    // for wrappers and non-MCP providers, and the section stays hidden.
    // Same rule for the handshake: a daemon that never advertised
    // `tool_policy` would refuse this request, so it is never sent.
    if (!toolPolicySupported || tools.length === 0) return;
    let cancelled = false;
    // Where the write sequence stood when this fetch started. A write issued
    // while the fetch is in flight is newer and owns the UI; a write that
    // settled before the fetch started does not poison it. Comparing against
    // the sequence at fetch start — never against zero — is what lets a
    // refetch (a reconnect's capability flip, a changed tool list) still
    // apply after a write.
    const seqAtFetch = seqRef.current;
    const writeWasInFlight = writesInFlightRef.current > 0;
    void toolPolicyGet()
      .then((reply) => {
        if (cancelled) return;
        // A write issued while this fetch was in flight is newer: keep it.
        if (seqRef.current !== seqAtFetch) return;
        // A write that was ALREADY in flight when the fetch started raced
        // it: whether the reply predates or postdates that write is
        // unknowable, so the reply adopts nothing — the write's own settle
        // (its optimistic row or its revert) is the state of record. The
        // guard answers "did a write overlap this fetch?", not just "is
        // there a newer write?".
        if (writeWasInFlight) return;
        policiesRef.current = reply.policies;
        setPolicies(reply.policies);
        // The store has spoken: a stale load error and its terminal state go.
        setError(null);
        setLoadFailed(false);
      })
      .catch((cause: unknown) => {
        if (!cancelled) {
          // Without stored rows there is nothing to show and nothing to
          // edit: that is a terminal state — the daemon's sentence plus a
          // Retry — not a loading state to sit under forever.
          if (policiesRef.current === null) setLoadFailed(true);
          setError(errorSentence(cause));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [provider.id, tools.length, toolPolicySupported, loadNonce]);
  if (!toolPolicySupported || tools.length === 0) return null;
  const { enabled, disabledTools } = toolPolicyFor(provider.id, policies);
  const disabledSet = new Set(disabledTools);
  // The stored rows are still in flight: until they land, `toolPolicyFor`
  // reads the missing row as "everything on", which is a guess, so nothing
  // in the card may be edited yet (finding 6 — the master switch was
  // already locked here while the tool rows below stayed live).
  const loading = policies === null;

  /**
   * The daemon never gates this tool, so it is never sent in a deny list
   * and a stored row that names it (stale daemon data) is stripped here.
   */
  function stripAlwaysOn(names: readonly string[]): string[] {
    return names.filter((name) => name !== ALWAYS_ON_TOOL);
  }

  async function persist(nextEnabled: boolean, nextDisabled: readonly string[]) {
    const cleanDisabled = stripAlwaysOn(nextDisabled);
    // This write's own revert base: the provider row as it stands right
    // now, read through the ref and not through the render closure, so an
    // earlier write's optimistic row is part of the base (findings 1, 8).
    const previous = toolPolicyFor(provider.id, policiesRef.current);
    // Sequence guard (findings 1, 8) — the card's real safety, stated here
    // and honoured by the controls: every write is sent immediately, in
    // click order; a second toggle must still reach the daemon, so the
    // controls stay reachable while a write is in flight and the only lock
    // is the load lock (`loading`) below. Overlap is resolved when a write
    // settles: only the newest sequence owns the UI, so a rejection a newer
    // write has superseded reverts nothing and reports nothing and the
    // newer optimistic row stands. (The agents panel — the twin that writes
    // a whole document whose row ids the daemon mints — serialises its
    // writes under one busy span instead, on purpose: an overlapping
    // whole-document write would re-send an id the daemon had already
    // replaced. Row-level toggles like these carry no minted identity, so
    // overlap is safe here and losing the click is not.)
    const seq = ++seqRef.current;
    writesInFlightRef.current += 1;
    setError(null);
    // Optimistic row, appended to the ref mirror: it always holds the
    // newest rows, including an earlier write's optimistic row when two
    // writes overlap.
    const row: ToolPolicyEntry = {
      providerId: provider.id,
      enabled: nextEnabled ? null : false,
      disabledTools: cleanDisabled,
    };
    const optimistic: readonly ToolPolicyEntry[] = [
      ...(policiesRef.current ?? []).filter((entry) => entry.providerId !== provider.id),
      row,
    ];
    policiesRef.current = optimistic;
    setPolicies(optimistic);
    try {
      await toolPolicySet(provider.id, nextEnabled ? null : false, cleanDisabled);
      // Confirmed. An older write settling here owns nothing: the newest
      // sequence keeps the UI, and there is no lock to release — the
      // controls were never locked against writes.
      return;
    } catch (cause) {
      // A newer write superseded this one: its optimistic row stands, this
      // rejection reports nothing.
      if (seq !== seqRef.current) return;
      // No newer write exists, so the row in the ref is the one this write
      // wrote: put back the row this write itself replaced, applied to the
      // current rows (never a stale render snapshot).
      const reverted: readonly ToolPolicyEntry[] = [
        ...(policiesRef.current ?? []).filter((entry) => entry.providerId !== provider.id),
        {
          providerId: provider.id,
          enabled: previous.enabled ? null : false,
          disabledTools: [...previous.disabledTools],
        },
      ];
      policiesRef.current = reverted;
      setPolicies(reverted);
      setError(errorSentence(cause));
    } finally {
      writesInFlightRef.current -= 1;
    }
  }

  function toggleProvider(next: boolean) {
    void persist(next, toolPolicyFor(provider.id, policiesRef.current).disabledTools);
  }

  function toggleTool(name: string, next: boolean) {
    if (name === ALWAYS_ON_TOOL) return;
    // Live state, not this render's: two toggles in one tick must each flip
    // the row the other just wrote rather than re-send a duplicate write.
    const current = toolPolicyFor(provider.id, policiesRef.current);
    const nextDisabled = next
      ? current.disabledTools.filter((tool) => tool !== name)
      : [...current.disabledTools, name];
    void persist(current.enabled, nextDisabled);
  }

  function retryLoad() {
    setError(null);
    setLoadFailed(false);
    setLoadNonce((nonce) => nonce + 1);
  }

  return (
    <div className="provider-card-block provider-tools">
      <details>
        <summary>Tool settings</summary>
        <label className="provider-tool-row">
          <input
            type="checkbox"
            role="switch"
            aria-label={`Enable tools for ${provider.id}`}
            checked={enabled}
            // Locked for the load only: a write in flight must not make the
            // controls unreachable — the sequence guard owns overlap (see
            // `persist`), and a control disabled on `busy` would drop the
            // user's second click, the one thing the policy says never
            // happens.
            disabled={loading}
            onChange={(event) => toggleProvider(event.target.checked)}
          />
          <span>Enable tools</span>
        </label>
        <div className="provider-tool-list">
          {tools.map((tool) => {
            const alwaysOn = tool.name === ALWAYS_ON_TOOL;
            const checked = alwaysOn ? true : enabled && !disabledSet.has(tool.name);
            const inputId = `tool-${provider.id}-${tool.name}`;
            return (
              <div className="provider-tool-row" key={tool.name}>
                <input
                  id={inputId}
                  type="checkbox"
                  checked={checked}
                  disabled={alwaysOn || !enabled || loading}
                  onChange={(event) => toggleTool(tool.name, event.target.checked)}
                />
                <label htmlFor={inputId}>
                  <span className="provider-tool-name">{tool.name}</span>
                  <span className="provider-tool-description"> {tool.description}</span>
                </label>
                {alwaysOn ? <span className="provider-tool-note">{ALWAYS_ON_REASON}</span> : null}
              </div>
            );
          })}
        </div>
        {error === null ? null : (
          <p role="alert" className="device-error">
            <ErrorText
              sentence={error.sentence}
              detail={error.detail}
              id="settings-tool-policy-error"
            />
          </p>
        )}
        {loadFailed ? (
          <button type="button" className="settings-device-action" onClick={retryLoad}>
            Retry
          </button>
        ) : null}
      </details>
    </div>
  );
}

export function ProvidersPanel() {
  const [catalog, setCatalog] = useState<ProviderCatalog | null>(null);
  const [error, setError] = useState<ErrorSentence | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // Bumped by every fetch (mount and refresh); a response only applies when its
  // sequence is still the latest, so a slow mount list cannot revert a refresh.
  const fetchSeqRef = useRef(0);
  // Set synchronously on click so a second click before the re-render is a no-op.
  const refreshInFlightRef = useRef(false);
  // The one npm run the daemon is executing on this client's behalf.
  const [npmRun, setNpmRun] = useState<ProviderNpmRun | null>(null);
  // Per-card, dismissible failure from the last npm run.
  const [npmFailure, setNpmFailure] = useState<{
    providerId: string;
    text: string;
    detail: string | null;
  } | null>(null);
  const [consent, setConsent] = useState<ProviderConsent | null>(null);
  // Cleared in the consent effect (not at the end of confirm): a second
  // synchronous click still sees the stale non-null consent, so the ref must
  // stay armed until that re-render.
  const consentInFlightRef = useRef(false);
  const consentConfirmRef = useRef<HTMLButtonElement>(null);
  const consentRestoreRef = useRef<HTMLButtonElement | null>(null);

  // The handshake's own capability list, through the same channel every other
  // surface reads it (Workspace, Design): the supervisor's `daemon_status`.
  // A daemon that never advertised `tool_policy` leaves the toggles off the
  // screen, so no card asks it for a policy it cannot answer.
  const daemon = useWorkspaceDaemon();
  const toolPolicySupported = daemon.capabilities.includes(TOOL_POLICY_CAPABILITY);

  useEffect(() => {
    consentInFlightRef.current = false;
    if (consent !== null) {
      consentConfirmRef.current?.focus();
    } else {
      consentRestoreRef.current?.focus();
      consentRestoreRef.current = null;
    }
  }, [consent]);

  useEffect(() => {
    if (consent === null) return;
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") setConsent(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [consent]);

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
        // "Refreshing…". Do not re-add an unmount guard here — StrictMode's
        // double mount kept a stale one true and wedged the button for real.
        refreshInFlightRef.current = false;
        setRefreshing(false);
      });
  }

  function openConsent(
    provider: ProviderInfo,
    verb: "update" | "install",
    trigger: HTMLButtonElement,
  ) {
    consentRestoreRef.current = trigger;
    setConsent({ provider, verb });
  }

  function confirmConsent() {
    if (consent === null || consentInFlightRef.current) return;
    consentInFlightRef.current = true;
    const { provider, verb } = consent;
    setConsent(null);
    setNpmFailure(null);
    setNpmRun({ providerId: provider.id, verb });
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

  const providers = catalog?.providers ?? null;
  const unreadableDirs = catalog?.unreadableDirs ?? 0;
  return (
    <div id="settings-panel-providers" role="tabpanel" aria-label="Providers and models">
      <SettingsHeading
        title="Providers & models"
        description="CLI agents found on PATH. An executable is not a login: the status shows the last measured start outcome, or unknown until one is measured."
      />
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
          <div className="provider-list" aria-busy={refreshing || npmRun !== null}>
            {providers.map((provider) => {
              const isNotInstalled = provider.installed === false;
              const runHere = npmRun?.providerId === provider.id;
              const consentHere = consent?.provider.id === provider.id;
              const failureHere = npmFailure?.providerId === provider.id;
              const detail = isNotInstalled
                ? (provider.npmPackage ?? provider.executable)
                : provider.executable;
              const npmCommand =
                consent !== null && consent.provider.npmPackage
                  ? `npm install -g ${consent.provider.npmPackage}@latest`
                  : null;
              return (
                <div
                  className="provider-card"
                  key={provider.id}
                  aria-busy={runHere ? "true" : undefined}
                >
                  <span className="provider-copy">
                    <span className="provider-name">{provider.id}</span>
                    {detail ? <span className="provider-detail">{detail}</span> : null}
                    <ProviderVersionLine provider={provider} />
                  </span>
                  <span className="provider-controls">
                    {isNotInstalled ? (
                      <span className="provider-status provider-status-idle">not installed</span>
                    ) : (
                      <>
                        {provider.origin === "npx-wrapper" ? (
                          <span className="provider-status provider-status-ready">npx</span>
                        ) : null}
                        {provider.protocol === "acp" ? (
                          <span className="provider-status provider-status-ready">ACP</span>
                        ) : provider.protocol === "stream-json" ? (
                          <span className="provider-status provider-status-ready">stream-json</span>
                        ) : provider.protocol === "pi-rpc" ? (
                          <span className="provider-status provider-status-ready">pi-rpc</span>
                        ) : provider.protocol === "codex-app-server" ? (
                          <span className="provider-status provider-status-ready">app-server</span>
                        ) : null}
                        <span
                          className={`provider-status ${
                            provider.authentication === "ok"
                              ? "provider-status-ready"
                              : provider.authentication.startsWith("failed:")
                                ? "provider-status-missing"
                                : "provider-status-idle"
                          }`}
                        >
                          {providerStatusText(provider)}
                        </span>
                      </>
                    )}
                    {runHere ? (
                      <button
                        className={`provider-refresh ${
                          npmRun.verb === "install" ? "provider-install" : "provider-update"
                        }`}
                        type="button"
                        disabled
                      >
                        {npmRun.verb === "install" ? "Installing…" : "Updating…"}
                      </button>
                    ) : (
                      <>
                        {!isNotInstalled && providerCanUpdate(provider) ? (
                          <button
                            className="provider-refresh provider-update"
                            type="button"
                            disabled={npmRun !== null}
                            onClick={(event) =>
                              openConsent(provider, "update", event.currentTarget)
                            }
                          >
                            Update
                          </button>
                        ) : null}
                        {isNotInstalled && provider.npmPackage ? (
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
                      </>
                    )}
                  </span>
                  {consentHere ? (
                    <div
                      className="provider-card-block provider-consent"
                      role="group"
                      aria-label={`Confirm ${consent.verb} for ${provider.id}`}
                    >
                      <div className="provider-consent-command">{npmCommand}</div>
                      <p className="provider-consent-notice">
                        This changes your global npm installation; running sessions keep the old
                        version until they are restarted.
                      </p>
                      <div className="provider-consent-actions">
                        <button
                          type="button"
                          className="provider-refresh provider-consent-cancel"
                          onClick={() => setConsent(null)}
                        >
                          Cancel
                        </button>
                        <button
                          ref={consentConfirmRef}
                          type="button"
                          className="provider-refresh provider-consent-confirm"
                          onClick={confirmConsent}
                        >
                          Confirm
                        </button>
                      </div>
                    </div>
                  ) : null}
                  {failureHere ? (
                    <div className="provider-card-block provider-update-error">
                      <pre
                        title={npmFailure.detail ?? undefined}
                        aria-describedby={
                          npmFailure.detail ? "settings-npm-failure-detail" : undefined
                        }
                      >
                        {npmFailure.text}
                        {npmFailure.detail ? (
                          <span id="settings-npm-failure-detail" className="error-detail-sr-only">
                            {npmFailure.detail}
                          </span>
                        ) : null}
                      </pre>
                      <button
                        type="button"
                        className="provider-refresh provider-update-error-dismiss"
                        onClick={() => setNpmFailure(null)}
                      >
                        Dismiss
                      </button>
                    </div>
                  ) : null}
                  <ProviderToolSettings
                    key={provider.id}
                    provider={provider}
                    toolPolicySupported={toolPolicySupported}
                  />
                </div>
              );
            })}
          </div>
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
