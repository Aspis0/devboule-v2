import { isValidElement, type ReactNode } from "react";
import { TableScrollRegion } from "./TableScrollRegion";
import { inline } from "./markdownInline";

type TableAlignment = "left" | "center" | "right" | null;

/** One pipe row's cells. An escaped `\|` joins the cell it sits in; a
 * line with no unescaped pipe is no row at all (`null`). The empty cells
 * an outer pipe opens are stripped from the ends, never from the middle. */
export function splitPipeRow(line: string): string[] | null {
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

export function alignmentOf(delimiterCell: string): TableAlignment {
  const cell = delimiterCell.trim();
  const left = cell.startsWith(":");
  const right = cell.endsWith(":");
  return left && right ? "center" : left ? "left" : right ? "right" : null;
}

/** GFM's ragged rule: a short row is padded with empty cells, a long
 * row's extras are dropped — the table always has the header's width. */
export function tableRowCells(row: string[], columns: number): string[] {
  const cells = row.map((cell) => cell.trim());
  return cells.length >= columns
    ? cells.slice(0, columns)
    : [...cells, ...Array<string>(columns - cells.length).fill("")];
}

/** A table starts here only when the line is a pipe row and the next one
 * is a delimiter row of the same cell count. Anything less — no
 * delimiter, a ragged one, no pipe at all (a setext heading's
 * `Title` / `---`) — is paragraphs, the way it read before tables. */
export function isTableStart(lines: string[], index: number): boolean {
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

export function tableBlock(
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
