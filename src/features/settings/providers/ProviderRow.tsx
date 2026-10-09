import { useRef, useState } from "react";
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

/** Muted version line on the row; renders nothing without data. */
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
   * Why no Log in item is offered: a no-workspace or no-command sentence,
   * shown under Advanced. Null when the item shows or when silence is correct.
   */
  loginHint: string | null;
  onConfirmConsent: () => void;
  onCancelConsent: () => void;
  onDismissFailure: () => void;
  onRefresh: () => void;
}

/**
 * One installed provider row: glyph, name, a status line (state, version,
 * npx, a running npm), the provider and Devboule-tools switches, the row menu
 * (Update, Log in, Refresh, copy path), and the Advanced toggle. Consent and
 * failure lines sit under the row while they need an answer. Path, protocol,
 * the model count, and the login hint live under Advanced. The vocabulary
 * probe mounts only once Advanced is open on an enabled row.
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
  // The protocol line names a protocol or renders nothing: on daemons that
  // report no protocol for a wrapper, "via npx" alone is not a protocol
  // and the group note plus the row word already carry provenance.
  const protocolName = provider.protocol ? protocolLabel(provider.protocol) : null;

  function openUpdateFromKebab() {
    setConsentFromKebab(true);
    onOpenUpdate(null);
  }

  function openLoginFromKebab() {
    setConsentFromKebab(true);
    onOpenLogin?.(null);
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
        <span className="prov-glyph">
          <ProviderGlyph providerId={provider.id} />
        </span>
        <span className="prov-main">
          <span className="prov-name">{provider.id}</span>
          <span className="prov-status" title={status.detail ?? undefined}>
            <span className={`prov-dot prov-dot-${status.tone}`} aria-hidden="true" />
            <span className="prov-status-word">{status.word}</span>
            {status.detail !== null ? <span className="sr-only">{status.detail}</span> : null}
            <ProviderVersionLine provider={provider} />
            {viaNpx ? <span className="prov-via">via npx</span> : null}
            {busyVerb !== null ? (
              <span className="prov-busy" role="status">
                {busyVerb === "update" ? "Updating…" : "Installing…"}
              </span>
            ) : null}
          </span>
        </span>
        <span className="prov-spacer" aria-hidden="true" />
        {providerSwitchSupported ? (
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
        <button
          type="button"
          className={`prov-chev${expanded ? " prov-chev-open" : ""}`}
          aria-expanded={expanded}
          {...(expanded ? { "aria-controls": detailsId } : {})}
          aria-label={`Advanced for ${provider.id}`}
          onClick={() => setExpanded((open) => !open)}
        >
          <span aria-hidden="true">›</span>
        </button>
      </div>
      {providerWriteError !== null ? (
        <div className="prov-row-line" role="alert">
          {providerWriteError}
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
        <div className="prov-row-line" role="status">
          <span className="prov-terminal-note">{terminalNotice.text}</span>
          {terminalNotice.lines.length > 0 ? <CopyableLines lines={terminalNotice.lines} /> : null}
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
      {expanded ? (
        <div className="prov-details" id={detailsId}>
          {!enabled ? (
            <div className="prov-detail-line" role="status">
              <span className="prov-terminal-note">Off. Existing sessions keep running.</span>
            </div>
          ) : null}
          <div className="prov-detail-line">
            <span className="prov-detail-label">Path</span>
            <code className="prov-detail-code">{provider.executable}</code>
          </div>
          {protocolName !== null ? (
            <div className="prov-detail-line">
              <span className="prov-detail-label">Protocol</span>
              <span className="prov-detail-value">{protocolName}</span>
            </div>
          ) : null}
          {enabled ? (
            <ProviderModelCount
              key={modelEpoch}
              providerId={provider.id}
              supported={vocabularySupported}
              cache={modelCache}
              epoch={modelEpoch}
            />
          ) : null}
          {loginHint !== null ? (
            <div className="prov-detail-line">
              <span className="prov-terminal-note">{loginHint}</span>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
