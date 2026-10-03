import { useCopyFeedback } from "../../../lib/useCopyFeedback";

export function MessageCopyButton({ text }: { text: string }) {
  const feedback = useCopyFeedback({ resetAfterMs: 1500 });
  const state = feedback.stateFor("message");

  const label = state === "copied" ? "Copied" : state === "failed" ? "Copy failed" : "Copy message";
  return (
    <button
      type="button"
      className="timeline-copy-chip"
      aria-label={label}
      onClick={() => void feedback.copy("message", text)}
    >
      {/* The word is the label, not the control: the button's own box is a
          24 px target, so only the state ever needs ink. */}
      {state === "failed" ? (
        "!"
      ) : (
        <svg
          width={14}
          height={14}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth={1.75}
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
          focusable="false"
        >
          {state === "copied" ? (
            <path d="M20 6 9 17l-5-5" />
          ) : (
            <>
              <rect x="9" y="9" width="11" height="11" rx="2" />
              <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
            </>
          )}
        </svg>
      )}
    </button>
  );
}
