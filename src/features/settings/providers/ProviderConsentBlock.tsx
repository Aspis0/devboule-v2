import { useEffect, useRef } from "react";

/**
 * The npm consent card shared by installed rows and available rows: the
 * exact command, the global-npm warning, Cancel / Confirm. Focus lands on
 * Confirm while the card lives; Escape cancels.
 */
export function ProviderConsentBlock({
  providerId,
  verb,
  command,
  onConfirm,
  onCancel,
}: {
  providerId: string;
  verb: "update" | "install";
  /** `npm install -g <package>@latest`, shown verbatim. */
  command: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const confirmRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    confirmRef.current?.focus();
  }, []);

  useEffect(() => {
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return (
    <div
      className="provider-card-block provider-consent"
      role="group"
      aria-label={`Confirm ${verb} for ${providerId}`}
    >
      <div className="provider-consent-command">{command}</div>
      <p className="provider-consent-notice">
        This changes your global npm installation; running sessions keep the old version until they
        are restarted.
      </p>
      <div className="provider-consent-actions">
        <button
          type="button"
          className="provider-refresh provider-consent-cancel"
          onClick={onCancel}
        >
          Cancel
        </button>
        <button
          ref={confirmRef}
          type="button"
          className="provider-refresh provider-consent-confirm"
          onClick={onConfirm}
        >
          Confirm
        </button>
      </div>
    </div>
  );
}
