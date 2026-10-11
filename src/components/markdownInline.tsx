import type { ReactNode } from "react";
import { ExternalLink } from "./ExternalLink";
import { parseChatCodeFilePath, scanChatFilePaths, type ChatFileLinks } from "../lib/chatFilePaths";
import { opensExternally } from "../lib/externalUrl";
import { endsInSeparator, localPathFromHref } from "../lib/markdownLocalPath";
import { FileLinkButton } from "./FileLinkButton";
import { workspaceRelativePath } from "../lib/workspaceRelativePath";

/** The workspace-relative spelling a candidate path names, or null when it is outside the workspace. */
function insideRoot(candidate: string, links: ChatFileLinks | null): string | null {
  return links === null ? null : workspaceRelativePath(candidate, links.root);
}

/**
 * A `[label](path)` naming a file: a button that opens it when the file is inside
 * the workspace, and the label alone otherwise. An agent's text cannot make the app
 * open a file outside the root. A folder is the label too: the Files tab cannot
 * open one yet.
 */
function localPathLink(
  key: number,
  label: string,
  path: string,
  links: ChatFileLinks | null,
): ReactNode {
  if (endsInSeparator(path)) return label;
  if (links === null) return <span key={key}>{label}</span>;
  const relative = insideRoot(path, links);
  if (relative === null) {
    return (
      <span key={key} title="Outside this workspace">
        {label}
      </span>
    );
  }
  return <FileLinkButton key={key} label={label} relative={relative} links={links} />;
}

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
    // A token outside the workspace is left in the plain text around it.
    const inside = (found ?? []).flatMap((token) => {
      const relative = insideRoot(text.slice(start + token.start, start + token.end), links);
      return relative === null ? [] : [{ token, relative }];
    });
    if (links === null || inside.length === 0) {
      nodes.push(plain(start, end));
      return;
    }
    let cursor = 0;
    for (const { token, relative } of inside) {
      if (token.start > cursor) nodes.push(plain(start + cursor, start + token.start));
      nodes.push(
        <FileLinkButton
          key={start + token.start}
          label={plain(start + token.start, start + token.end)}
          relative={relative}
          links={links}
        />,
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
      const displayed = links === null ? null : parseChatCodeFilePath(content, links.root);
      const relative = displayed === null ? null : insideRoot(content, links);
      nodes.push(
        links === null || relative === null ? (
          <code key={key}>{links === null ? content : content.replace(/\n/g, " ")}</code>
        ) : (
          <code key={key}>
            <FileLinkButton label={content} relative={relative} links={links} />
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
      // An http(s) destination the command would refuse stays the text the agent
      // wrote: a link that opens nothing is worse than none, and hiding the URL
      // protects nothing that the raw message, journal and copy do not show.
      const localPath = localPathFromHref(href);
      if (opensExternally(href) || /^mailto:/i.test(href)) {
        nodes.push(
          <ExternalLink key={key} href={href}>
            {label}
          </ExternalLink>,
        );
      } else if (localPath !== null) {
        nodes.push(localPathLink(key, label, localPath, links));
      } else if (/^https?:/i.test(href)) {
        nodes.push(text.slice(index, end));
      } else {
        // Not a path or a web address: the label the person reads, never the raw markdown.
        nodes.push(label);
      }
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
