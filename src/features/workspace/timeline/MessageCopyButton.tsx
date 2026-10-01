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
      {state === "copied" ? "✓" : state === "failed" ? "!" : "Copy"}
    </button>
  );
}
