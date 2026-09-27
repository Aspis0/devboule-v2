import { useState } from "react";

/** One exact line the person can copy to paste elsewhere. */
export interface CopyableLine {
  /** Whose shell this line is for; null when there is only one line. */
  label?: string | null;
  text: string;
}

/**
 * Each exact line with its own Copy button. Feedback stays on the button
 * until the next copy or unmount — no timers, nothing to outlive the menu
 * or card that hosts it (same rule as the kebab's copy item).
 */
export function CopyableLines({ lines }: { lines: ReadonlyArray<CopyableLine> }) {
  const [copied, setCopied] = useState<number | null>(null);
  const [failed, setFailed] = useState<number | null>(null);

  async function copy(index: number, text: string) {
    try {
      const clipboard = (
        navigator as Navigator & {
          clipboard?: { writeText: (text: string) => Promise<void> };
        }
      ).clipboard;
      if (!clipboard) throw new Error("no clipboard in this host");
      await clipboard.writeText(text);
      setCopied(index);
      setFailed(null);
    } catch {
      setFailed(index);
      setCopied(null);
    }
  }

  return (
    <div className="provider-copy-lines">
      {lines.map((line, index) => (
        <div className="provider-copy-line" key={line.text}>
          {line.label ? <span className="provider-copy-label">{line.label}</span> : null}
          <code className="provider-consent-command">{line.text}</code>
          <button
            type="button"
            className="provider-refresh provider-copy-button"
            onClick={() => void copy(index, line.text)}
          >
            {failed === index ? "Copy failed" : copied === index ? "Copied" : "Copy"}
          </button>
        </div>
      ))}
    </div>
  );
}
