import type { ProviderInfo } from "../../../types/ipc";
import { CopyableLines, type CopyableLine } from "./CopyableLines";
import { ProviderConsentBlock } from "./ProviderConsentBlock";
import { ProviderNpmFailure } from "./ProviderNpmFailure";
import { ProviderVersionLine, type ProviderRowConsent } from "./ProviderRow";
import { SHELL_QUERY_LOADING } from "./terminalShell";
import { providerInstallPackage } from "./providerTerminalCommands";

/** One catalogue row that is not installed yet: its id, package, version, and Install. */
export function ProviderAvailableRow({
  provider,
  busyVerb,
  actionsDisabled,
  notice,
  consent,
  npmFailure,
  onInstall,
  onDismissNotice,
  onConfirm,
  onCancel,
  onDismissFailure,
}: {
  provider: ProviderInfo;
  /** This row's npm run, while the daemon executes it. */
  busyVerb: "update" | "install" | null;
  /** Another row's npm run holds the daemon: no Install anywhere. */
  actionsDisabled: boolean;
  /** A terminal handoff recorded for this row, shown until dismissed. */
  notice: { text: string; lines: ReadonlyArray<CopyableLine> } | null;
  /** The open consent for this row, or "waiting" while the shell report is in flight. */
  consent: ProviderRowConsent | "waiting" | null;
  npmFailure: { text: string; detail: string | null } | null;
  onInstall: (trigger: HTMLButtonElement) => void;
  onDismissNotice: () => void;
  onConfirm: () => void;
  onCancel: () => void;
  onDismissFailure: () => void;
}) {
  // Install is offered only when a safe line exists: the package check refuses
  // names outside the strict shape. The shell it will be typed for is resolved at open.
  const installable = providerInstallPackage(provider) !== null;
  return (
    <div className="prov-available-row" tabIndex={-1} data-provider-row={provider.id}>
      <span className="prov-available-main">
        <span className="prov-name">{provider.id}</span>
        {(provider.npmPackage ?? provider.executable) ? (
          <span className="prov-available-sub">{provider.npmPackage ?? provider.executable}</span>
        ) : null}
        <ProviderVersionLine provider={provider} />
      </span>
      {notice !== null ? (
        <span className="provider-card-block prov-terminal-note" role="status">
          {notice.text}
          {notice.lines.length > 0 ? <CopyableLines lines={notice.lines} /> : null}
          <button
            type="button"
            className="provider-refresh provider-update-error-dismiss"
            onClick={onDismissNotice}
          >
            Dismiss
          </button>
        </span>
      ) : busyVerb !== null ? (
        <span className="prov-busy" role="status">
          {busyVerb === "install" ? "Installing…" : "Updating…"}
        </span>
      ) : installable ? (
        <button
          className="provider-refresh provider-install"
          type="button"
          disabled={actionsDisabled}
          onClick={(event) => onInstall(event.currentTarget)}
        >
          Install
        </button>
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
              onClick={onCancel}
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
          onConfirm={onConfirm}
          onCancel={onCancel}
        />
      ) : null}
      {npmFailure !== null ? (
        <ProviderNpmFailure
          text={npmFailure.text}
          detail={npmFailure.detail}
          onDismiss={onDismissFailure}
        />
      ) : null}
    </div>
  );
}
