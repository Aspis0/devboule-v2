import { memo, useCallback, useState } from "react";
import { copyToClipboard } from "../lib/clipboard";
import { stripTrailingBlankLines } from "../lib/fence";
import "./codeBlocks.css";

function BlockCopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);

  const copy = useCallback(async () => {
    if (await copyToClipboard(text)) setCopied(true);
  }, [text]);

  return (
    <>
      <button
        type="button"
        className={copied ? "copy-btn is-copied" : "copy-btn"}
        aria-label={copied ? "Copied" : "Copy code"}
        onClick={() => void copy()}
      >
        {copied ? "✓ Copied" : "Copy"}
      </button>
      <span aria-live="polite" className="sr-only">
        {copied ? "Copied" : ""}
      </span>
    </>
  );
}

const MemoizedBlockCopyButton = memo(BlockCopyButton);

export function CodeBlock({ code, copyable }: { code: string; copyable: boolean }) {
  const content = stripTrailingBlankLines(code);
  if (content === "") return null;
  const button = <MemoizedBlockCopyButton text={content} />;
  if (copyable) {
    return (
      <div className="copyblock">
        {button}
        {code}
      </div>
    );
  }
  return (
    <div className="codeblock-sample">
      {button}
      <pre>
        <code>{code}</code>
      </pre>
    </div>
  );
}
