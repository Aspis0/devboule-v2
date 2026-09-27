import { useCallback, useEffect, useRef, useState } from "react";
import type { ProviderInfo } from "../../../types/ipc";
import type { ErrorSentence } from "../../../lib/errorSentence";
import { providerCanUpdate, providerRowStatus, providerVersionSegments } from "../providerStatus";
import { ProviderGlyph } from "./ProviderGlyph";
import { ProviderKebab } from "./ProviderKebab";
import { ProviderModelCount, type ModelCountCache } from "./ProviderModelCount";
import { ProviderToolsSwitch } from "./ProviderToolsSwitch";
import { ProviderConsentBlock } from "./ProviderConsentBlock";
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

export interface ProviderRowProps {
  provider: ProviderInfo;
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
  consentOpen: boolean;
  npmCommand: string | null;
  npmVerb: "update" | "install" | null;
  npmFailure: { text: string; detail: string | null } | null;
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
  /** Panel-owned node registry for returning focus after Confirm. */
  rowRef: (node: HTMLDivElement | null) => void;
  onToggleTools: (next: boolean) => void;
  onTurnAllOn: () => void;
  onOpenUpdate: (trigger: HTMLButtonElement | null) => void;
  onConfirmConsent: () => void;
  onCancelConsent: () => void;
  onDismissFailure: () => void;
  onRefresh: () => void;
}

/**
 * One installed provider row: h44 — chevron, glyph, sans name, dot status,
 * the single Devboule-tools switch, kebab. The chevron opens the details
 * (path, version, protocol, update); the model count mounts only there, and
 * only after a measured start, so the panel never probes on mount.
 */
export function ProviderRow({
  provider,
  toolPolicy,
  toolsDisabled,
  vocabularySupported,
  modelCache,
  consentOpen,
  npmCommand,
  npmVerb,
  npmFailure,
  writeError,
  onDismissWriteError,
  busyVerb,
  actionsDisabled,
  modelEpoch,
  viaNpx,
  rowRef,
  onToggleTools,
  onTurnAllOn,
  onOpenUpdate,
  onConfirmConsent,
  onCancelConsent,
  onDismissFailure,
  onRefresh,
}: ProviderRowProps) {
  const [expanded, setExpanded] = useState(false);
  const rowScopeRef = useRef<HTMLDivElement>(null);
  const setWrapNode = useCallback(
    (node: HTMLDivElement | null) => {
      rowScopeRef.current = node;
      rowRef(node);
    },
    [rowRef],
  );
  // Kebab Update passes no trigger (the menu item unmounts), so the row
  // remembers where consent came from and returns focus to the kebab on
  // Cancel itself — the panel's restore effect only covers live triggers.
  const [consentFromKebab, setConsentFromKebab] = useState(false);
  // Consent, failure, write errors, and a running npm live inside the
  // details: arriving any of them opens the row, so a kebab Update on a
  // collapsed row still reveals its consent card instead of opening it
  // invisibly.
  useEffect(() => {
    if (consentOpen || npmFailure !== null || writeError !== null || busyVerb !== null) {
      setExpanded(true);
    }
  }, [consentOpen, npmFailure, writeError, busyVerb]);
  const status = providerRowStatus(provider);
  const canUpdate = providerCanUpdate(provider) && !actionsDisabled;
  // Provider ids are user-declarable (`user_providers` rows), so the id is
  // sanitised before it becomes a DOM id.
  const detailsId = `prov-details-${provider.id.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
  const hasVersion = providerVersionSegments(provider).length > 0;
  const protocolParts = [
    provider.protocol ? protocolLabel(provider.protocol) : null,
    provider.origin === "npx-wrapper" ? "via npx" : null,
  ].filter((part): part is string => part !== null);

  function openUpdateFromKebab() {
    setConsentFromKebab(true);
    onOpenUpdate(null);
  }

  function openUpdateFromDetails(trigger: HTMLButtonElement) {
    setConsentFromKebab(false);
    onOpenUpdate(trigger);
  }

  function cancelConsent() {
    onCancelConsent();
    if (consentFromKebab) {
      rowScopeRef.current?.querySelector<HTMLButtonElement>(".prov-kebab")?.focus();
    }
  }

  return (
    <div className="prov-row-wrap" ref={setWrapNode} tabIndex={-1} data-provider-row={provider.id}>
      <div className="prov-row">
        <button
          type="button"
          className={`prov-chev${expanded ? " prov-chev-open" : ""}`}
          aria-expanded={expanded}
          {...(expanded ? { "aria-controls": detailsId } : {})}
          aria-label={`Details for ${provider.id}`}
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
          {expanded ? (
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
        {toolPolicy === null ? null : (
          <ProviderToolsSwitch
            providerId={provider.id}
            enabled={toolPolicy.enabled}
            hasLegacyDenials={toolPolicy.disabledTools.length > 0}
            disabled={toolsDisabled}
            onToggle={onToggleTools}
            onTurnAllOn={onTurnAllOn}
          />
        )}
        <ProviderKebab
          providerId={provider.id}
          path={provider.executable}
          onUpdate={canUpdate ? openUpdateFromKebab : undefined}
          onRefresh={onRefresh}
        />
      </div>
      {expanded ? (
        <div className="prov-details" id={detailsId}>
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
          {protocolParts.length > 0 ? (
            <div className="prov-detail-line">
              <span className="prov-detail-label">Protocol</span>
              <span className="prov-detail-value">{protocolParts.join(" · ")}</span>
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
          {consentOpen && npmCommand !== null && npmVerb !== null ? (
            <ProviderConsentBlock
              providerId={provider.id}
              verb={npmVerb}
              command={npmCommand}
              onConfirm={onConfirmConsent}
              onCancel={cancelConsent}
            />
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
