import { useEffect, useRef } from "react";

/**
 * A terminal tab the daemon would not start. Focus lands on Dismiss when it
 * appears: the person has to acknowledge the refusal before anything else.
 */
export function ProvidersTerminalError({
  error,
  onDismiss,
}: {
  /** Identity matters: a new refusal object re-takes focus even while this block is up. */
  error: { providerId: string; text: string; detail: string | null };
  onDismiss: () => void;
}) {
  const dismissRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    dismissRef.current?.focus();
  }, [error]);
  const { providerId, text, detail } = error;
  return (
    <div role="alert" className="provider-card-block provider-update-error">
      <span
        title={detail ?? undefined}
        aria-describedby={detail ? "settings-terminal-error-detail" : undefined}
      >
        {providerId}: {text}
        {detail ? (
          <span id="settings-terminal-error-detail" className="error-detail-sr-only">
            {detail}
          </span>
        ) : null}
      </span>
      <button
        ref={dismissRef}
        type="button"
        className="provider-refresh provider-update-error-dismiss"
        onClick={onDismiss}
      >
        Dismiss
      </button>
    </div>
  );
}
