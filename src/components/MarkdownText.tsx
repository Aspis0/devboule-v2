import { memo } from "react";
import { parseMarkdownText } from "./markdownParser";

export const MarkdownText = memo(function MarkdownText({ text }: { text: string }) {
  return <div>{parseMarkdownText(text)}</div>;
});
