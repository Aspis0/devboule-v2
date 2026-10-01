import type { ReactNode } from "react";
import { parseChatCodeFilePath, scanChatFilePaths, type ChatFileLinks } from "../lib/chatFilePaths";

export function inline(text: string, fileLinks?: ChatFileLinks | null): ReactNode[] {
  const links = fileLinks ?? null;
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
  const plain = (start: number, end: number) => {
    const content = text.slice(start, end).replace(/\\([!-/:-@[-`{-~])/g, "$1");
    return links === null ? content : content.replace(/\n/g, " ");
  };
  const pushPlain = (start: number, end: number) => {
    const found = links === null ? null : scanChatFilePaths(text.slice(start, end), links.root);
    if (found === null || found.length === 0) {
      nodes.push(plain(start, end));
      return;
    }
    let cursor = 0;
    for (const token of found) {
      if (token.start > cursor) nodes.push(plain(start + cursor, start + token.start));
      nodes.push(
        <button
          type="button"
          key={start + token.start}
          className="plan-markdown-file-link"
          title={token.link.relativePath}
          onClick={() => links?.open(token.link.relativePath)}
        >
          {plain(start + token.start, start + token.end)}
        </button>,
      );
      cursor = token.end;
    }
    if (cursor < end - start) nodes.push(plain(start + cursor, end));
  };
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
    if (index > renderedUntil) pushPlain(renderedUntil, index);
    const key = index;
    const content = text.slice(contentStart, contentEnd);
    if (kind === "code") {
      const link = links === null ? null : parseChatCodeFilePath(content, links.root);
      nodes.push(
        link === null ? (
          <code key={key}>{links === null ? content : content.replace(/\n/g, " ")}</code>
        ) : (
          <code key={key}>
            <button
              type="button"
              className="plan-markdown-file-link"
              title={link.relativePath}
              onClick={() => links?.open(link.relativePath)}
            >
              {content}
            </button>
          </code>
        ),
      );
    }
    // Emphasis content ends at its first emphasis star, so this nested scan
    // finds code and links but never another emphasis: no depth cap needed.
    else if (kind === "strong") nodes.push(<strong key={key}>{inline(content, links)}</strong>);
    else if (kind === "em") nodes.push(<em key={key}>{inline(content, links)}</em>);
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
  if (renderedUntil === 0) {
    pushPlain(0, text.length);
    return nodes;
  }
  if (renderedUntil < text.length) pushPlain(renderedUntil, text.length);
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
