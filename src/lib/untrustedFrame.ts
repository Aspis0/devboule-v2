/**
 * The daemon's untrusted-content frame, as the person's view leaves it out.
 *
 * The frame is for the model: it names where content came from and where it
 * ends. The journal keeps it (a replay that re-sends the text to a model must
 * still carry it); a row that shows the text to the person drops the fixed
 * block and keeps every word the content said. The two shapes are the ones
 * `untrusted_frame.rs` writes: a lead-in block before content that runs to the
 * end of the message, and a head block and tail line around a tool result.
 * A tool result can sit after other text the row already holds, and text can
 * follow its tail, so the fence is found wherever it stands. A frame still
 * arriving, its tail not yet written, is dropped down to its head.
 */

const FRAME_OPEN = "[devboule: untrusted content]";
const LEAD_IN_END = "The content is everything after this block, to the end of the message.";
/** The first words of every line a frame's header is written with, and of its fence head. */
const HEADER_STARTS = [
  "source: ",
  "provenance: ",
  "chain: ",
  "trust: ",
  "The content ",
  "content-begin ",
];

/** The lead-in: header lines (never a `content-begin`), then its closing line. */
const LEAD_IN = new RegExp(
  `${escapeRegExp(FRAME_OPEN)}\\n(?:(?!content-begin )[^\\n]*\\n)*?${escapeRegExp(LEAD_IN_END)}`,
);
/** A fence head, carrying the nonce its own tail must repeat. */
const FENCE_HEAD = new RegExp(
  `${escapeRegExp(FRAME_OPEN)}\\n(?:[^\\n]*\\n)*?content-begin ([0-9a-f]{16})\\n?`,
);

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** `text` without the daemon's frame, or unchanged when it holds none. */
export function hideUntrustedFrame(text: string): string {
  if (!text.includes(FRAME_OPEN)) return text;
  const head = FENCE_HEAD.exec(text);
  if (head !== null) return hideFence(text, head);
  const lead = LEAD_IN.exec(text);
  if (lead !== null) {
    const before = text.slice(0, lead.index).replace(/\n+$/, "");
    const after = text.slice(lead.index + lead[0].length).replace(/^\n+/, "");
    return [before, after].filter((part) => part.length > 0).join("\n\n");
  }
  return hideStreamingHeader(text);
}

/** A fenced result: its head goes, and its content stays, up to the tail with the head's own nonce. */
function hideFence(text: string, head: RegExpExecArray): string {
  const start = head.index;
  const rest = text.slice(start + head[0].length);
  const tail = new RegExp(`(?:^|\\n)content-end ${head[1]}(?=\\n|$)`).exec(rest);
  if (tail === null) return text.slice(0, start) + rest;
  const content = rest.slice(0, tail.index);
  const after = rest.slice(tail.index + tail[0].length);
  return text.slice(0, start) + content + after;
}

/**
 * A frame whose header is still arriving: every complete line is a header line
 * and the last one is the start of one. Anything else after the marker is the
 * person's own text, so it stays.
 */
function hideStreamingHeader(text: string): string {
  const open = text.indexOf(FRAME_OPEN);
  const rest = text.slice(open + FRAME_OPEN.length);
  if (rest !== "" && !rest.startsWith("\n")) return text;
  const lines = rest.slice(1).split("\n");
  const partial = lines.pop() ?? "";
  const headerOnly = lines.every((line) => HEADER_STARTS.some((start) => line.startsWith(start)));
  const partialOk =
    partial === "" ||
    HEADER_STARTS.some((start) => start.startsWith(partial) || partial.startsWith(start));
  if (!headerOnly || !partialOk) return text;
  return text.slice(0, open).replace(/\n+$/, "");
}
