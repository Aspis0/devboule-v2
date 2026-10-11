import type { ReactNode } from "react";
import { ErrorTriangleIcon } from "../ErrorTriangleIcon";

/**
 * One quiet line: the row's words, a failure's triangle in the icon slot. Its
 * detail waits behind the click when there is one; an action on the line (opening
 * a child) makes the whole line a button. A line with neither is plain text.
 */
export function QuietLine({
  failed = false,
  detail,
  onOpen,
  children,
}: {
  failed?: boolean;
  detail?: ReactNode;
  onOpen?: () => void;
  children: ReactNode;
}) {
  const line = (
    <>
      {failed ? <ErrorTriangleIcon /> : null}
      <span className="workspace-chat-tool-label">{children}</span>
    </>
  );
  if (onOpen !== undefined) {
    return (
      <button
        type="button"
        className="workspace-chat-tool-summary workspace-chat-quiet-line"
        onClick={onOpen}
      >
        {line}
      </button>
    );
  }
  if (detail === undefined) {
    return <div className="workspace-chat-tool-summary workspace-chat-quiet-line">{line}</div>;
  }
  return (
    <details className="workspace-chat-tool-details workspace-chat-quiet-line">
      <summary className="workspace-chat-tool-summary">{line}</summary>
      <div className="workspace-chat-tool-body">{detail}</div>
    </details>
  );
}
