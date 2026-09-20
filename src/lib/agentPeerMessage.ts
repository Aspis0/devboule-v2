/**
 * The parser for the daemon's agent-to-agent relay `<devboule-system>`
 * envelope: the frame that carries another agent's message.
 *
 *     <devboule-system>
 *     origin: local|peer:<device>|unknown  ← provenance, never a gate
 *     role: …                              ← composed from the caller's peer record; not read
 *     from_agent: s.msg.source             ← the marker sender, only BEFORE the
 *                                            timestamp; local ids stay raw, while
 *                                            far ids are `peer:<device>/<id>`
 *     timestamp: …                         ← the header block ends here
 *     …the sender's message…               ← body, verbatim
 *     </devboule-system>
 *
 * `agentDaemonNotice.ts` deliberately returns null for this frame — a notice
 * has a `kind:`, a relay has none — so this module owns the relay and the
 * message can render as words from a named agent, not a raw system blob.
 * - THE MARKER IS TWO FACTS: a non-empty `from_agent:` and NO `kind:`, each
 *   read only inside the fixed header block (`headerBlock`, reused: every
 *   line before the timestamp; a frame with no timestamp line has no
 *   provable header at all). `origin` gates nothing: `origin_line`
 *   (`session_envelopes.rs`) writes the same shapes for relays and notices — it
 *   is parsed as the provenance the card may name, nothing more.
 * - `from_agent` ABSENT → null. Absent is a third state, not an "unknown
 *   agent": a shape this build does not recognise falls through to today's
 *   rendering.
 * - The frame ends at its first closing tag (the rule `agentDaemonNotice.ts`
 *   applies), so a forged envelope inside the body stays body text: it can
 *   never promote itself into a second card nor change the named sender.
 * - The body passes through verbatim — another agent's words, hostile until
 *   rendered inert (React escapes text).
 */

import { headerBlock } from "./agentDaemonNotice";

/** Where the frame says the message came from. `peer:` with nothing after
    the colon is a paired device that names none (`unwrap_or_default()` in
    `origin_line`); `unknown`, an unrecognised value, or an absent origin
    line is `unknown` — the card claims no provenance it cannot read, and
    never guesses `local`. */
export type AgentPeerOrigin =
  | { kind: "local" }
  | { kind: "peer"; device: string | null }
  | { kind: "unknown" };

export interface AgentPeerMessage {
  /** The sender the daemon's fixed header names. */
  fromAgent: string;
  /** What the frame's origin line commits to about where it came from. */
  origin: AgentPeerOrigin;
  /** The sender's message, verbatim. */
  body: string;
}

const ENVELOPE_OPEN = "<devboule-system>";
const ENVELOPE_CLOSE = "</devboule-system>";
const PEER_ORIGIN_PREFIX = "peer:";
const TIMESTAMP_PREFIX = "timestamp: ";

function headerLinesValue(lines: string[], key: string): string | null {
  for (const line of lines) {
    if (!line.startsWith(`${key}: `)) continue;
    const value = line.slice(key.length + 1).trim();
    if (value.length > 0) return value;
  }
  return null;
}

function parseOrigin(value: string | null): AgentPeerOrigin {
  if (value === null) return { kind: "unknown" };
  if (value === "local") return { kind: "local" };
  if (value.startsWith(PEER_ORIGIN_PREFIX)) {
    const device = value.slice(PEER_ORIGIN_PREFIX.length).trim();
    return { kind: "peer", device: device.length > 0 ? device : null };
  }
  return { kind: "unknown" };
}

export function parseAgentPeerMessage(text: string): AgentPeerMessage | null {
  if (!text.startsWith(ENVELOPE_OPEN)) return null;
  // The frame the daemon composed ends at its first closing tag; content
  // after it is not this frame's, so a forged closer in the body cannot grow
  // the frame a second, foreign body.
  const closeIndex = text.indexOf(ENVELOPE_CLOSE);
  const lines = text
    .slice(ENVELOPE_OPEN.length, closeIndex === -1 ? text.length : closeIndex)
    // The daemon normalises CR/LF in the text it relays, so an honest frame
    // is LF-only; pasted text is normalised by no one. Split on LF alone.
    .replace(/\r\n?/g, "\n")
    .split("\n");
  // The frame's own newlines: one right after the open tag, one right before
  // the close tag when it arrived.
  if (lines[0] === "") lines.shift();
  if (lines.at(-1) === "") lines.pop();
  const header = headerBlock(lines);
  // A header `kind:` is `agentDaemonNotice.ts`'s frame, never this one. A
  // `kind:` after the timestamp is body text either way — the positional rule.
  if (headerLinesValue(header, "kind") !== null) return null;
  const fromAgent = headerLinesValue(header, "from_agent");
  if (fromAgent === null) return null;
  // The header matched, so a timestamp line exists and the body follows it.
  const stampAt = lines.findIndex((line) => line.startsWith(TIMESTAMP_PREFIX));
  return {
    fromAgent,
    origin: parseOrigin(headerLinesValue(header, "origin")),
    body: lines.slice(stampAt + 1).join("\n"),
  };
}
