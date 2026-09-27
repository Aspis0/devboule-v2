import type { ReactNode } from "react";

function inline(text: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const next = (character: string) => {
    const positions = new Int32Array(text.length + 1);
    positions[text.length] = -1;
    for (let index = text.length - 1; index >= 0; index -= 1) {
      positions[index] = text[index] === character ? index : positions[index + 1];
    }
    return positions;
  };
  const ticks = next("`");
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
    if (text[index] === "`" && ticks[index + 1] > index + 1) {
      kind = "code";
      end = ticks[index + 1];
      contentEnd = end;
    } else if (text.startsWith("**", index) && stars[index + 2] > index + 2) {
      kind = "strong";
      contentStart = index + 2;
      contentEnd = stars[index + 2];
      end = contentEnd + 2;
      if (!text.startsWith("**", contentEnd)) end = -1;
    } else if (text[index] === "*" && stars[index + 1] > index + 1) {
      kind = "em";
      contentEnd = stars[index + 1];
      end = contentEnd + 1;
    } else if (text[index] === "[" && brackets[index + 1] > index + 1) {
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
    if (index > renderedUntil) nodes.push(text.slice(renderedUntil, index));
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
  if (renderedUntil === 0) return [text];
  if (renderedUntil < text.length) nodes.push(text.slice(renderedUntil));
  return nodes;
}

export function MarkdownText({ text }: { text: string }) {
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
      blocks.push(
        <pre key={blocks.length}>
          <code>{code.join("\n")}</code>
        </pre>,
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
  return <div>{blocks}</div>;
}
