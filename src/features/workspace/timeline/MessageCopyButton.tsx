import { useEffect, useRef, useState } from "react";
import { copyToClipboard } from "../../../lib/clipboard";

type CopyState = "ready" | "copied" | "failed";

export function MessageCopyButton({ text }: { text: string }) {
  const [state, setState] = useState<CopyState>("ready");
  const resetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (resetTimer.current !== null) clearTimeout(resetTimer.current);
    },
    [],
  );

  async function copyMessage() {
    if (resetTimer.current !== null) clearTimeout(resetTimer.current);
    setState((await copyToClipboard(text)) ? "copied" : "failed");
    resetTimer.current = setTimeout(() => setState("ready"), 1500);
  }

  const label = state === "copied" ? "Copied" : state === "failed" ? "Copy failed" : "Copy message";
  return (
    <button
      type="button"
      className="timeline-copy-chip"
      aria-label={label}
      onClick={() => void copyMessage()}
    >
      {state === "copied" ? "✓" : state === "failed" ? "!" : "Copy"}
    </button>
  );
}
