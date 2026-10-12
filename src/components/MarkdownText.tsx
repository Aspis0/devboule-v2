import { memo } from "react";
import type { ChatFileLinks } from "../lib/chatFilePaths";
import { parseMarkdownText } from "./markdownParser";
import type { MarkdownImageRenderer } from "./markdownInline";

export const MarkdownText = memo(function MarkdownText({
  text,
  fileLinks = null,
  renderImage,
}: {
  text: string;
  /** Workspace context that turns agent-written paths into open links.
   * Null keeps every other surface's rendering unchanged. */
  fileLinks?: ChatFileLinks | null;
  /** Draws a markdown image; absent leaves it as its alt text. */
  renderImage?: MarkdownImageRenderer;
}) {
  return <div>{parseMarkdownText(text, fileLinks, renderImage)}</div>;
});
