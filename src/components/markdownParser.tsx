import type { ReactNode } from "react";
import { CodeBlock } from "./CodeBlock";
import { isCopyableFence } from "../lib/fence";

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
  const parens = next(")");
  let offset = 0;
  let renderedUntil = 0;
  while (offset < text.length) {
    const index = offset;
    let end = -1;
    let kind: "code" | "strong" | "em" | "link" | undefined;
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
    } else if (text[index] === "[" && escaped[index] === 0 && brackets[index + 1] > index + 1) {
      const close = brackets[index + 1];
      if (text[close + 1] === "(" && parens[close + 2] > close + 2) {
        kind = "link";
        contentEnd = close;
        end = parens[close + 2] + 1;
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
    const paragraph = [line.trim()];
    index += 1;
    while (
      index < lines.length &&
      lines[index].trim() !== "" &&
      !/^(#{1,6})\s+/.test(lines[index]) &&
      !/^```/.test(lines[index]) &&
      !/^\s*[-*+]\s+/.test(lines[index]) &&
      !/^\s*\d+[.)]\s+/.test(lines[index])
    ) {
      paragraph.push(lines[index].trim());
      index += 1;
    }
    blocks.push(<p key={blocks.length}>{inline(paragraph.join(" "))}</p>);
  }
  return blocks;
}
