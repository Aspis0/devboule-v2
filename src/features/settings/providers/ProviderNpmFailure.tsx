import { ErrorText } from "../../../components/ErrorText";
import type { ErrorSentence } from "../../../lib/errorSentence";

/** The dismissible npm tail shown under the row that ran it. */
export function ProviderNpmFailure({
  text,
  detail,
  onDismiss,
}: {
  text: string;
  detail: string | null;
  onDismiss: () => void;
}) {
  return (
    <div className="provider-card-block provider-update-error">
      <pre
        title={detail ?? undefined}
        aria-describedby={detail ? "settings-npm-failure-detail" : undefined}
      >
        {text}
        {detail ? (
          <span id="settings-npm-failure-detail" className="error-detail-sr-only">
            {detail}
          </span>
        ) : null}
      </pre>
      <button
        type="button"
        className="provider-refresh provider-update-error-dismiss"
        onClick={onDismiss}
      >
        Dismiss
      </button>
    </div>
  );
}

/** A rejected single-switch write, shown inside its own row. */
export function ProviderWriteError({
  error,
  providerId,
}: {
  error: ErrorSentence;
  providerId: string;
}) {
  return (
    <p role="alert" className="device-error">
      <ErrorText
        sentence={error.sentence}
        detail={error.detail}
        id={`settings-tool-policy-error-${providerId}`}
      />
    </p>
  );
}
