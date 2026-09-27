import { useEffect, useRef, useState } from "react";
import type { ProviderInfo } from "../../../types/ipc";
import type { ErrorSentence } from "../../../lib/errorSentence";
import { providerCanUpdate, providerRowStatus, providerVersionSegments } from "../providerStatus";
import { ProviderGlyph } from "./ProviderGlyph";
import { ProviderKebab } from "./ProviderKebab";
import { ProviderModelCount, type ModelCountCache } from "./ProviderModelCount";
import { ProviderToolsSwitch } from "./ProviderToolsSwitch";
import { ProviderConsentBlock } from "./ProviderConsentBlock";
import { CopyableLines, type CopyableLine } from "./CopyableLines";
import { SHELL_QUERY_LOADING } from "./terminalShell";
import { ProviderNpmFailure, ProviderWriteError } from "./ProviderNpmFailure";

/** Muted version line for the expanded details; renders nothing without data. */
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

/** Today's protocol chips, kept verbatim: they name what the daemon measured. */
function protocolLabel(protocol: string): string {
  if (protocol === "acp") return "ACP";
  if (protocol === "stream-json") return "stream-json";
  if (protocol === "pi-rpc") return "pi-rpc";
  if (protocol === "codex-app-server") return "app-server";
  return protocol;
}

/** What an open consent shows: verbatim lines, copy lines, or nothing. */
export interface ProviderRowConsent {
  verb: "update" | "install" | "login";
  /** The exact lines, shown verbatim in type order. */
  lines: readonly string[];
  /** Exact lines to copy instead of auto-typing (unknown shell). */
  copyLines: ReadonlyArray<CopyableLine> | null;
  /** The warning under the lines; null when the lines stand alone. */
  notice: string | null;
}

export interface ProviderRowProps {
  provider: ProviderInfo;
  enabled: boolean;
  onToggleProvider: (next: boolean) => void;
  providerWriteError: string | null;
  providerSwitchSupported: boolean;
  /**
   * The stored tool-policy row, or null when no switch is shown: either the
   * handshake did not advertise `tool_policy`, or this provider serves no
   * Devboule tools (`provider.tools` empty).
   */
  toolPolicy: { enabled: boolean; disabledTools: readonly string[] } | null;
  /** The load lock: nothing may be edited from a guess. */
  toolsDisabled: boolean;
  vocabularySupported: boolean;
  modelCache: ModelCountCache;
  /**
   * The open consent for this row, or "waiting" while the shell report is
   * in flight. Null renders nothing.
   */
  consent: ProviderRowConsent | "waiting" | null;
  npmFailure: { text: string; detail: string | null } | null;
  /**
   * A terminal handoff waiting on this row, shown until Refresh or dismiss.
   * Lines are empty for a plain handoff and carry the exact lines to copy
   * when nothing was typed (expired take, or a tab opened untyped).
   */
  terminalNotice: { text: string; lines: ReadonlyArray<CopyableLine> } | null;
  onDismissNotice: () => void;
  writeError: ErrorSentence | null;
  onDismissWriteError: () => void;
  /** This row's npm run, while the daemon executes it. */
  busyVerb: "update" | "install" | null;
  /** Another row's npm run holds the daemon: no Update anywhere. */
  actionsDisabled: boolean;
  /** Bumped by Refresh: remounts the lazy count so an open row re-reads. */
  modelEpoch: number;
  /** Registry row that starts through npx: said plainly on the row. */
  viaNpx: boolean;
  onToggleTools: (next: boolean) => void;
  onTurnAllOn: () => void;
  onOpenUpdate: (trigger: HTMLButtonElement | null) => void;
  /** Absent when the provider documents no login command: no entry points. */
  onOpenLogin?: (trigger: HTMLButtonElement | null) => void;
  /**
   * Why no Log in button is shown: a no-workspace or no-command sentence.
   * Null when the button shows or when silence is correct (never both).
   */
  loginHint: string | null;
  onConfirmConsent: () => void;
  onCancelConsent: () => void;
  onDismissFailure: () => void;
  onRefresh: () => void;
}

/**
 * One installed provider row: h44 — chevron, glyph, name, dot status, the
 * provider and Devboule-tools switches, and kebab. The chevron opens details
 * (path, version, protocol, update); vocabulary mounts only in an expanded,
 * enabled row, so opening a switched-off row never probes its provider.
 */
export function ProviderRow({
  provider,
  enabled,
  onToggleProvider,
  providerWriteError,
  providerSwitchSupported,
  toolPolicy,
  toolsDisabled,
  vocabularySupported,
  modelCache,
  consent,
  npmFailure,
  terminalNotice,
  onDismissNotice,
  writeError,
  onDismissWriteError,
  busyVerb,
  actionsDisabled,
  modelEpoch,
  viaNpx,
  onToggleTools,
  onTurnAllOn,
  onOpenUpdate,
  onOpenLogin,
  loginHint,
  onConfirmConsent,
  onCancelConsent,
  onDismissFailure,
  onRefresh,
}: ProviderRowProps) {
  const [expanded, setExpanded] = useState(false);
  const rowScopeRef = useRef<HTMLDivElement>(null);
  // Kebab Update passes no trigger (the menu item unmounts), so the row
  // remembers where consent came from and returns focus to the kebab on
  // Cancel itself — the panel's restore effect only covers live triggers.
  const [consentFromKebab, setConsentFromKebab] = useState(false);
  // Consent, failure, write errors, a running npm, and a terminal handoff
  // live inside the details: arriving any of them opens the row, so a
  // kebab Update on a collapsed row still reveals its consent card instead
  // of opening it invisibly.
  useEffect(() => {
    if (
      consent !== null ||
      npmFailure !== null ||
      writeError !== null ||
      providerWriteError !== null ||
      busyVerb !== null ||
      terminalNotice !== null
    ) {
      setExpanded(true);
    }
  }, [consent, npmFailure, writeError, providerWriteError, busyVerb, terminalNotice]);
  const detailsOpen = expanded || !enabled;
  const status = enabled
    ? providerRowStatus(provider)
    : { tone: "idle" as const, word: "Off", detail: null };
  const canUpdate = providerCanUpdate(provider) && !actionsDisabled;
  // A login consent types into a terminal tab, independent of the daemon's
  // npm lock — but the consent card is still one per page, so a running
  // npm keeps every entry point shut.
  const canLogin = enabled && onOpenLogin !== undefined && !actionsDisabled;
  // Provider ids are user-declarable (`user_providers` rows), so the id is
  // sanitised before it becomes a DOM id.
  const detailsId = `prov-details-${provider.id.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
  const hasVersion = providerVersionSegments(provider).length > 0;
  // The protocol line names a protocol or renders nothing: on daemons that
  // report no protocol for a wrapper, "via npx" alone is not a protocol
  // and the group note plus the row word already carry provenance.
  const protocolName = provider.protocol ? protocolLabel(provider.protocol) : null;

  function openUpdateFromKebab() {
    setConsentFromKebab(true);
    onOpenUpdate(null);
  }

  function openUpdateFromDetails(trigger: HTMLButtonElement) {
    setConsentFromKebab(false);
    onOpenUpdate(trigger);
  }

  function openLoginFromKebab() {
    setConsentFromKebab(true);
    onOpenLogin?.(null);
  }

  function openLoginFromDetails(trigger: HTMLButtonElement) {
    setConsentFromKebab(false);
    onOpenLogin?.(trigger);
  }

  function cancelConsent() {
    onCancelConsent();
    if (consentFromKebab) {
      rowScopeRef.current?.querySelector<HTMLButtonElement>(".prov-kebab")?.focus();
    }
  }

  return (
    <div className="prov-row-wrap" ref={rowScopeRef} tabIndex={-1} data-provider-row={provider.id}>
      <div className="prov-row">
        <button
          type="button"
          className={`prov-chev${detailsOpen ? " prov-chev-open" : ""}`}
          aria-expanded={detailsOpen}
          {...(detailsOpen ? { "aria-controls": detailsId } : {})}
          aria-label={`Details for ${provider.id}`}
          disabled={!enabled}
          onClick={() => setExpanded((open) => !open)}
        >
          <span aria-hidden="true">›</span>
        </button>
        <span className="prov-glyph">
          <ProviderGlyph providerId={provider.id} />
        </span>
        <span className="prov-name">{provider.id}</span>
        <span className="prov-status" title={status.detail ?? undefined}>
          <span className={`prov-dot prov-dot-${status.tone}`} aria-hidden="true" />
          <span className="prov-status-word">{status.word}</span>
          {status.detail !== null ? <span className="sr-only">{status.detail}</span> : null}
          {viaNpx ? <span className="prov-via">via npx</span> : null}
          {detailsOpen && enabled ? (
            <ProviderModelCount
              key={modelEpoch}
              providerId={provider.id}
              supported={vocabularySupported}
              cache={modelCache}
              epoch={modelEpoch}
            />
          ) : null}
        </span>
        <span className="prov-spacer" aria-hidden="true" />
        {providerSwitchSupported ? (
          <span className="prov-tools">
            <span className="prov-tools-label" aria-hidden="true">
              On
            </span>
            <button
              type="button"
              role="switch"
              aria-checked={enabled}
              aria-label={`On for ${provider.id}`}
              className={`prov-switch${enabled ? " prov-switch-on" : ""}`}
              onClick={() => onToggleProvider(!enabled)}
            >
              <i aria-hidden="true" />
            </button>
          </span>
        ) : null}
        {toolPolicy === null ? null : (
          <ProviderToolsSwitch
            providerId={provider.id}
            enabled={toolPolicy.enabled}
            hasLegacyDenials={toolPolicy.disabledTools.length > 0}
            disabled={toolsDisabled || !enabled}
            onToggle={onToggleTools}
            onTurnAllOn={onTurnAllOn}
          />
        )}
        <ProviderKebab
          providerId={provider.id}
          path={provider.executable}
          onUpdate={canUpdate ? openUpdateFromKebab : undefined}
          onLogin={canLogin ? openLoginFromKebab : undefined}
          onRefresh={onRefresh}
        />
      </div>
      {detailsOpen ? (
        <div className="prov-details" id={detailsId}>
          {!enabled ? (
            <div className="prov-detail-line" role="status">
              <span className="prov-terminal-note">Off. Existing sessions keep running.</span>
            </div>
          ) : null}
          {providerWriteError !== null ? (
            <div className="prov-detail-line" role="alert">
              {providerWriteError}
            </div>
          ) : null}
          <div className="prov-detail-line">
            <span className="prov-detail-label">Path</span>
            <code className="prov-detail-code">{provider.executable}</code>
          </div>
          {hasVersion ? (
            <div className="prov-detail-line">
              <span className="prov-detail-label">Version</span>
              <ProviderVersionLine provider={provider} />
            </div>
          ) : null}
          {protocolName !== null ? (
            <div className="prov-detail-line">
              <span className="prov-detail-label">Protocol</span>
              <span className="prov-detail-value">{protocolName}</span>
            </div>
          ) : null}
          {busyVerb !== null ? (
            <div className="prov-detail-line" role="status">
              <span className="prov-busy">
                {busyVerb === "update" ? "Updating…" : "Installing…"}
              </span>
            </div>
          ) : null}
          {canUpdate ? (
            <div className="prov-detail-line">
              <button
                type="button"
                className="provider-refresh provider-update"
                onClick={(event) => openUpdateFromDetails(event.currentTarget)}
              >
                Update
              </button>
            </div>
          ) : null}
          {canLogin ? (
            <div className="prov-detail-line">
              <button
                type="button"
                className="provider-refresh provider-login"
                onClick={(event) => openLoginFromDetails(event.currentTarget)}
              >
                Log in
              </button>
            </div>
          ) : loginHint !== null ? (
            <div className="prov-detail-line">
              <span className="prov-terminal-note">{loginHint}</span>
            </div>
          ) : null}
          {consent === "waiting" ? (
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
                  onClick={cancelConsent}
                >
                  Cancel
                </button>
              </div>
            </div>
          ) : consent !== null ? (
            <ProviderConsentBlock
              providerId={provider.id}
              verb={consent.verb}
              lines={consent.lines}
              copyLines={consent.copyLines}
              notice={consent.notice}
              onConfirm={onConfirmConsent}
              onCancel={cancelConsent}
            />
          ) : null}
          {terminalNotice !== null ? (
            <div className="prov-detail-line" role="status">
              <span className="prov-terminal-note">{terminalNotice.text}</span>
              {terminalNotice.lines.length > 0 ? (
                <CopyableLines lines={terminalNotice.lines} />
              ) : null}
              <button
                type="button"
                className="provider-refresh provider-update-error-dismiss"
                onClick={onDismissNotice}
              >
                Dismiss
              </button>
            </div>
          ) : null}
          {npmFailure !== null ? (
            <ProviderNpmFailure
              text={npmFailure.text}
              detail={npmFailure.detail}
              onDismiss={onDismissFailure}
            />
          ) : null}
          {writeError === null ? null : (
            <ProviderWriteError
              error={writeError}
              providerId={provider.id}
              onDismiss={onDismissWriteError}
            />
          )}
        </div>
      ) : null}
    </div>
  );
}
