import { ALWAYS_ON_REASON } from "../providerStatus";

/**
 * The row's only switch: Devboule tools for this provider, on or off. The
 * label names the tools so the switch never reads as provider on/off (that
 * toggle needs a daemon config and lives in slice R17-1b, not here).
 */
export function ProviderToolsSwitch({
  providerId,
  enabled,
  hasLegacyDenials,
  disabled,
  onToggle,
  onTurnAllOn,
}: {
  providerId: string;
  /** The stored row's boolean: absent row reads as on. */
  enabled: boolean;
  /** Stored row still denies tools invisibly while reading on. */
  hasLegacyDenials: boolean;
  /** The load lock: nothing may be edited from a guess. */
  disabled: boolean;
  onToggle: (next: boolean) => void;
  onTurnAllOn: () => void;
}) {
  const showLegacy = enabled && hasLegacyDenials;
  return (
    <span className="prov-tools">
      <span className="prov-tools-label" aria-hidden="true">
        Devboule tools
      </span>
      <button
        type="button"
        role="switch"
        aria-checked={enabled}
        aria-label={`Devboule tools for ${providerId}`}
        title={ALWAYS_ON_REASON}
        className={`prov-switch${enabled ? " prov-switch-on" : ""}`}
        disabled={disabled}
        onClick={() => onToggle(!enabled)}
      >
        <i aria-hidden="true" />
      </button>
      {showLegacy ? (
        <span className="prov-legacy" role="status">
          Some Devboule tools are off from an older setting.
          <button type="button" className="prov-legacy-action" onClick={onTurnAllOn}>
            Turn all on
          </button>
        </span>
      ) : null}
    </span>
  );
}
