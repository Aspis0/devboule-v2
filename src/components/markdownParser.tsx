import type { ReactNode } from "react";
import { CodeBlock } from "./CodeBlock";
import { inline } from "./markdownInline";
import {
  alignmentOf,
  isTableStart,
  splitPipeRow,
  tableBlock,
  tableRowCells,
} from "./markdownTables";
import { isCopyableFence } from "../lib/fence";
import "./markdown.css";

export function parseMarkdownText(text: string): ReactNode[] {
  const eol = text.includes("\r\n") ? "\r\n" : "\n";
  const lines = text.split(/\r?\n/);
  const blocks: ReactNode[] = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index];
    if (line.trim() === "") {
      index += 1;
      continue;
    }
    const fence = /^```/.test(line);
    if (fence) {
      const code: string[] = [];
      index += 1;
      while (index < lines.length && !/^```/.test(lines[index])) code.push(lines[index++]);
      if (index < lines.length) index += 1;
      const body = code.join(eol);
      blocks.push(
        <CodeBlock
          key={blocks.length}
          code={body}
          copyable={isCopyableFence(line.slice(3).trim(), body)}
        />,
      );
      continue;
    }
    const heading = /^(#{1,6})\s+(.+)$/.exec(line);
    if (heading) {
      blocks.push(
        <div
          key={blocks.length}
          role="heading"
          aria-level={heading[1].length}
          className={`plan-markdown-heading plan-markdown-heading-${heading[1].length}`}
        >
          {inline(heading[2])}
        </div>,
      );
      index += 1;
      continue;
    }
    if (/^\s*[-*+]\s+/.test(line)) {
      const items: ReactNode[] = [];
      while (index < lines.length && /^\s*[-*+]\s+/.test(lines[index])) {
        items.push(<li key={items.length}>{inline(lines[index].replace(/^\s*[-*+]\s+/, ""))}</li>);
        index += 1;
      }
      blocks.push(<ul key={blocks.length}>{items}</ul>);
      continue;
    }
    if (/^\s*\d+[.)]\s+/.test(line)) {
      const items: ReactNode[] = [];
      while (index < lines.length && /^\s*\d+[.)]\s+/.test(lines[index])) {
        items.push(
          <li key={items.length}>{inline(lines[index].replace(/^\s*\d+[.)]\s+/, ""))}</li>,
        );
        index += 1;
      }
      blocks.push(<ol key={blocks.length}>{items}</ol>);
      continue;
    }
    if (isTableStart(lines, index)) {
      const headerRow = splitPipeRow(lines[index]) ?? [];
      const columns = headerRow.length;
      const alignments = (splitPipeRow(lines[index + 1]) ?? []).map(alignmentOf);
      index += 2;
      const rows: string[][] = [];
      while (index < lines.length) {
        const row = splitPipeRow(lines[index]);
        if (row === null) break;
        rows.push(tableRowCells(row, columns));
        index += 1;
      }
      blocks.push(tableBlock(blocks.length, tableRowCells(headerRow, columns), alignments, rows));
      continue;
    }
    const paragraph = [line.trim()];
    index += 1;
    while (
      index < lines.length &&
      lines[index].trim() !== "" &&
      !/^(#{1,6})\s+/.test(lines[index]) &&
      !/^```/.test(lines[index]) &&
      !/^\s*[-*+]\s+/.test(lines[index]) &&
      !/^\s*\d+[.)]\s+/.test(lines[index]) &&
      // A table interrupts the paragraph it would otherwise be swallowed
      // by: the loop stops at a header-plus-delimiter pair so the block
      // loop above sees the table on its next pass.
      !isTableStart(lines, index)
    ) {
      paragraph.push(lines[index].trim());
      index += 1;
    }
    blocks.push(<p key={blocks.length}>{inline(paragraph.join(" "))}</p>);
  }
  return blocks;
}
