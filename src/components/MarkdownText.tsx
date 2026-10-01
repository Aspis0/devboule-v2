import { memo } from "react";
import type { ChatFileLinks } from "../lib/chatFilePaths";
import { parseMarkdownText } from "./markdownParser";

export const MarkdownText = memo(function MarkdownText({
  text,
  fileLinks = null,
}: {
  text: string;
  /** Workspace context that turns agent-written paths into open links.
   * Null keeps every other surface's rendering unchanged. */
  fileLinks?: ChatFileLinks | null;
}) {
  return <div>{parseMarkdownText(text, fileLinks)}</div>;
});
