import type { ReactNode } from "react";
import { ErrorTriangleIcon } from "../ErrorTriangleIcon";

/**
 * One quiet line: the row's words, a failure's triangle in the icon slot, and
 * the detail behind the click. Without a detail the line is not a button.
 */
export function QuietLine({
  failed = false,
  detail,
  children,
}: {
  failed?: boolean;
  detail?: ReactNode;
  children: ReactNode;
}) {
  const line = (
    <>
      {failed ? <ErrorTriangleIcon /> : null}
      <span className="workspace-chat-tool-label">{children}</span>
    </>
  );
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
