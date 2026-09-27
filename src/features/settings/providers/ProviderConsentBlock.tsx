import { useEffect, useRef } from "react";

/**
 * The consent card shared by installed rows and available rows: the exact
 * lines, the warning, Cancel / Confirm. One line per div so a terminal
 * install shows its install line and its login line separately — Windows
 * PowerShell 5.1 cannot parse `&&`, so what the card shows is what gets
 * typed, one line plus Enter at a time. Focus lands on Confirm while the
 * card lives; Escape cancels.
 */
export function ProviderConsentBlock({
  providerId,
  verb,
  lines,
  notice,
  onConfirm,
  onCancel,
}: {
  providerId: string;
  verb: "update" | "install" | "login";
  /** Shown verbatim, in order. */
  lines: readonly string[];
  /** The warning under the lines; null when the lines speak for themselves. */
  notice: string | null;
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
      {lines.map((line) => (
        <div className="provider-consent-command" key={line}>
          {line}
        </div>
      ))}
      {notice !== null ? <p className="provider-consent-notice">{notice}</p> : null}
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
