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
 * follow its tail, so the fence is found wherever it stands.
 */

const FRAME_OPEN = "[devboule: untrusted content]";
const LEAD_IN_END = "The content is everything after this block, to the end of the message.";

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

/** `text` without the daemon's frame, or unchanged when it is not provably one. */
export function hideUntrustedFrame(text: string): string {
  if (!text.includes(FRAME_OPEN)) return text;
  const head = FENCE_HEAD.exec(text);
  if (head !== null) return hideFence(text, head);
  const lead = LEAD_IN.exec(text);
  if (lead === null) return text;
  const before = text.slice(0, lead.index).replace(/\n+$/, "");
  const after = text.slice(lead.index + lead[0].length).replace(/^\n+/, "");
  return [before, after].filter((part) => part.length > 0).join("\n\n");
}

/**
 * A fenced result without its tail is left alone. With one, the first tail that
 * repeats the head's nonce closes the content: the text around the frame stays,
 * and a tail line with any other nonce is content, not a close.
 */
function hideFence(text: string, head: RegExpExecArray): string {
  const start = head.index;
  const rest = text.slice(start + head[0].length);
  const tail = new RegExp(`(?:^|\\n)content-end ${head[1]}(?=\\n|$)`).exec(rest);
  if (tail === null) return text;
  const content = rest.slice(0, tail.index);
  const after = rest.slice(tail.index + tail[0].length);
  return text.slice(0, start) + content + after;
}
