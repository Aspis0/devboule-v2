import { isValidElement, type ReactNode } from "react";
import { CodeBlock } from "./CodeBlock";
import { TableScrollRegion } from "./TableScrollRegion";
import { isCopyableFence } from "../lib/fence";
import "./markdown.css";

function inline(text: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const escaped = new Uint8Array(text.length);
  const codeSpan = new Uint8Array(text.length);
  const codeEnd = new Int32Array(text.length);
  codeEnd.fill(-1);
  let codeStart = -1;
  for (let index = 0; index < text.length; index += 1) {
    if (codeStart >= 0) {
      if (text[index] === "`") {
        codeEnd[codeStart] = index;
        codeSpan.fill(1, codeStart, index + 1);
        codeStart = -1;
      }
      continue;
    }
    if (text[index] === "\\" && isMarkdownPunctuation(text.charCodeAt(index + 1))) {
      escaped[index + 1] = 1;
      index += 1;
    } else if (text[index] === "`") {
      codeStart = index;
    }
  }
  if (codeStart >= 0) {
    for (let index = codeStart + 1; index < text.length; index += 1) {
      if (text[index] === "\\" && isMarkdownPunctuation(text.charCodeAt(index + 1))) {
        escaped[index + 1] = 1;
        index += 1;
      }
    }
  }
  const next = (character: string) => {
    const positions = new Int32Array(text.length + 1);
    positions[text.length] = -1;
    for (let index = text.length - 1; index >= 0; index -= 1) {
      positions[index] =
        text[index] === character && escaped[index] === 0 && codeSpan[index] === 0
          ? index
          : positions[index + 1];
    }
    return positions;
  };
  const plain = (start: number, end: number) =>
    text.slice(start, end).replace(/\\([!-/:-@[-`{-~])/g, "$1");
  const stars = next("*");
  const brackets = next("]");
  // Every `(`'s matching `)` in one stack pass: a target's own parens stay
  // inside it, so `http://e/wiki/A_(B)` keeps its full href. Escaped and
  // code-span parens never close; an open that never closes stays -1.
  const parenClose = new Int32Array(text.length).fill(-1);
  const opens = new Int32Array(text.length);
  let depth = 0;
  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];
    if ((character === "(" || character === ")") && escaped[index] === 0 && codeSpan[index] === 0) {
      if (character === "(") {
        opens[depth] = index;
        depth += 1;
      } else if (depth > 0) {
        depth -= 1;
        parenClose[opens[depth]] = index;
      }
    }
  }
  // A destination is cut at the first stop, so a URL carrying a raw space or `<` is deliberately not a link.
  const nextStop = new Int32Array(text.length + 1);
  nextStop[text.length] = text.length;
  for (let index = text.length - 1; index >= 0; index -= 1) {
    const character = text[index];
    nextStop[index] =
      character === " " ||
      character === "\t" ||
      character === "\r" ||
      character === "\n" ||
      character === "\f" ||
      character === "<"
        ? index
        : nextStop[index + 1];
  }
  const destinationEnd = (close: number): number => {
    const targetEnd = parenClose[close + 1];
    return targetEnd > close + 2 && nextStop[close + 2] >= targetEnd ? targetEnd : -1;
  };
  let offset = 0;
  let renderedUntil = 0;
  while (offset < text.length) {
    const index = offset;
    let end = -1;
    let kind: "code" | "strong" | "em" | "link" | "image" | undefined;
    let contentStart = index + 1;
    let contentEnd = -1;
    if (text[index] === "`" && codeEnd[index] >= 0) {
      kind = "code";
      contentEnd = codeEnd[index];
      end = contentEnd + 1;
    } else if (
      text.startsWith("**", index) &&
      escaped[index] === 0 &&
      escaped[index + 1] === 0 &&
      stars[index + 2] > index + 2
    ) {
      kind = "strong";
      contentStart = index + 2;
      contentEnd = stars[index + 2];
      end = contentEnd + 2;
      if (!text.startsWith("**", contentEnd)) end = -1;
    } else if (text[index] === "*" && escaped[index] === 0 && stars[index + 1] > index + 1) {
      kind = "em";
      contentEnd = stars[index + 1];
      end = contentEnd + 1;
    } else if (
      // An image is its alt text and nothing else: no element loads the
      // URL, so a relative path can never be fetched and a remote one is
      // never requested.
      text[index] === "!" &&
      text[index + 1] === "[" &&
      escaped[index] === 0 &&
      brackets[index + 2] >= index + 2
    ) {
      const close = brackets[index + 2];
      if (text[close + 1] === "(") {
        const targetEnd = destinationEnd(close);
        if (targetEnd !== -1) {
          kind = "image";
          contentStart = index + 2;
          contentEnd = close;
          end = targetEnd + 1;
        }
      }
    } else if (text[index] === "[" && escaped[index] === 0 && brackets[index + 1] > index + 1) {
      const close = brackets[index + 1];
      if (text[close + 1] === "(") {
        const targetEnd = destinationEnd(close);
        if (targetEnd !== -1) {
          kind = "link";
          contentEnd = close;
          end = targetEnd + 1;
        }
      }
    }
    if (kind === undefined || end <= index) {
      offset += 1;
      continue;
    }
    if (index > renderedUntil) nodes.push(plain(renderedUntil, index));
    const key = index;
    const content = text.slice(contentStart, contentEnd);
    if (kind === "code") nodes.push(<code key={key}>{content}</code>);
    else if (kind === "strong") nodes.push(<strong key={key}>{content}</strong>);
    else if (kind === "em") nodes.push(<em key={key}>{content}</em>);
    else if (kind === "image")
      nodes.push(
        <span key={key} className="plan-markdown-image">
          {content === "" ? "image" : content}
        </span>,
      );
    else {
      const href = text.slice(contentEnd + 2, end - 1);
      const label = text.slice(contentStart, contentEnd);
      nodes.push(
        /^(https?:|mailto:)/i.test(href) ? (
          <a key={key} href={href} target="_blank" rel="noreferrer">
            {label}
          </a>
        ) : (
          text.slice(index, end)
        ),
      );
    }
    renderedUntil = end;
    offset = end;
  }
  if (renderedUntil === 0) return [plain(0, text.length)];
  if (renderedUntil < text.length) nodes.push(plain(renderedUntil, text.length));
  return nodes;
}

function isMarkdownPunctuation(code: number): boolean {
  return (
    (code >= 33 && code <= 47) ||
    (code >= 58 && code <= 64) ||
    (code >= 91 && code <= 96) ||
    (code >= 123 && code <= 126)
  );
}

type TableAlignment = "left" | "center" | "right" | null;

/** One pipe row's cells. An escaped `\|` joins the cell it sits in; a
 * line with no unescaped pipe is no row at all (`null`). The empty cells
 * an outer pipe opens are stripped from the ends, never from the middle. */
function splitPipeRow(line: string): string[] | null {
  let hasPipe = false;
  const cells: string[] = [];
  let cell = "";
  for (let index = 0; index < line.length; index += 1) {
    const character = line[index];
    if (character === "\\" && line[index + 1] === "|") {
      cell += "|";
      index += 1;
    } else if (character === "|") {
      hasPipe = true;
      cells.push(cell);
      cell = "";
    } else {
      cell += character;
    }
  }
  cells.push(cell);
  if (!hasPipe) return null;
  if (cells.length > 1 && cells[0].trim() === "") cells.shift();
  if (cells.length > 1 && cells[cells.length - 1].trim() === "") cells.pop();
  return cells;
}

function isDelimiterRow(cells: string[]): boolean {
  return cells.length > 0 && cells.every((cell) => /^:?-+:?$/.test(cell.trim()));
}

function alignmentOf(delimiterCell: string): TableAlignment {
  const cell = delimiterCell.trim();
  const left = cell.startsWith(":");
  const right = cell.endsWith(":");
  return left && right ? "center" : left ? "left" : right ? "right" : null;
}

/** GFM's ragged rule: a short row is padded with empty cells, a long
 * row's extras are dropped — the table always has the header's width. */
function tableRowCells(row: string[], columns: number): string[] {
  const cells = row.map((cell) => cell.trim());
  return cells.length >= columns
    ? cells.slice(0, columns)
    : [...cells, ...Array<string>(columns - cells.length).fill("")];
}

/** A table starts here only when the line is a pipe row and the next one
 * is a delimiter row of the same cell count. Anything less — no
 * delimiter, a ragged one, no pipe at all (a setext heading's
 * `Title` / `---`) — is paragraphs, the way it read before tables. */
function isTableStart(lines: string[], index: number): boolean {
  const header = splitPipeRow(lines[index]);
  if (header === null || index + 1 >= lines.length) return false;
  const delimiter = splitPipeRow(lines[index + 1]);
  return delimiter !== null && isDelimiterRow(delimiter) && delimiter.length === header.length;
}

// The region's name is the <th>'s own text: walking the rendered nodes keeps them identical.
function textOf(node: ReactNode): string {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (isValidElement<{ children?: ReactNode; alt?: unknown }>(node)) {
    if (node.type === "img" && typeof node.props.alt === "string") return node.props.alt;
    return textOf(node.props.children);
  }
  return "";
}

function tableBlock(
  key: number,
  headers: string[],
  alignments: TableAlignment[],
  rows: string[][],
): ReactNode {
  const align = (column: number) =>
    alignments[column] === undefined || alignments[column] === null
      ? undefined
      : { textAlign: alignments[column]! };
  const headerNodes = headers.map((cell) => inline(cell));
  return (
    <TableScrollRegion key={key} headers={headerNodes.map((nodes) => textOf(nodes).trim())}>
      <table className="plan-markdown-table">
        <thead>
          <tr>
            {headerNodes.map((nodes, column) => (
              <th key={column} style={align(column)}>
                {nodes}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, rowIndex) => (
            <tr key={rowIndex}>
              {row.map((cell, column) => (
                <td key={column} style={align(column)}>
                  {inline(cell)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </TableScrollRegion>
  );
}

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
