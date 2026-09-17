/**
 * The parser for the daemon's `<devboule-system>` notice envelopes other than
 * the permission request (which `agentPermissionRequest.ts` owns): the frames
 * that claim `role: daemon` and report what a created child is doing —
 * `agent_finished`, `agent_input_required`, `agent_quiet` — plus any kind a
 * newer daemon adds that this build has never heard of.
 *
 *     <devboule-system>
 *     origin: …
 *     role: daemon          ← necessary, not sufficient
 *     from_agent: …
 *     kind: …               ← the marker, and only BEFORE the timestamp
 *     timestamp: …          ← the header block ends here
 *     …kind-specific fields…
 *     </devboule-system>
 *
 * - POSITIONAL KIND GATE: the daemon composes the fixed header — origin,
 *   role, from_agent, kind, timestamp — before any caller-controlled byte,
 *   while an agent-to-agent echo carries the caller's free text after the
 *   timestamp line and composes `role:` from the caller's peer record. A
 *   `kind:` line counts only inside that header block. See `headerBlock`.
 * - `role:` ALONE DOES NOT MARK A NOTICE. The daemon composes it from the
 *   CALLER's peer record (`session.rs:5574`), so an echo sent by a session a
 *   paired daemon created reads `role: daemon` while carrying another
 *   agent's words. The marker is a `kind:` line in the fixed header: all
 *   four notices carry one, the echo carries none. A frame missing either
 *   returns null and is rendered by author, with its text intact.
 * - A frame that carries both NEVER returns null — recognized kind, or
 *   `recognized: false` naming the kind it declared — not even when the
 *   daemon's size bound cut the closing tag off (then `truncated`). A cut
 *   that lands inside the header removes the timestamp line, which empties
 *   the header block, so such a frame returns null rather than half-parsing.
 * - A recognized kind demotes when a field its card cannot stand without is
 *   missing or malformed; the surface invents no stand-in values.
 * - THE FINISH TAIL IS NOT ATTRIBUTABLE HERE: the child's summary, the
 *   daemon's note and the artifacts line are written unfenced, and the
 *   child's prose may imitate them. Everything from the first
 *   `note:`/`artifacts:` line to the frame's final `artifacts:` line travels
 *   as `unattributed` — a block that claims neither voice. A daemon-side
 *   fence for the summary would close this properly.
 * - For an unknown kind it names only what the fixed header commits to: the
 *   kind line and the `from_agent` pointer.
 * - Values pass through verbatim: no un-escaping, no re-truncation.
 *   Rendering stays inert (React escapes text).
 */

export type AgentDaemonNotice =
  | {
      recognized: true;
      kind: "agent_finished";
      childSessionId: string;
      /** Absent stays absent — the surface never invents a name. */
      childName: string | null;
      state: string | null;
      /** The child's own words, provably: the summary up to its first field
          marker. The surface quotes them; never daemon-styled. */
      summary: string | null;
      /** The frame's tail from its first field marker on — the daemon's note
          and the summary's continuation are not tellable apart here. */
      unattributed: string | null;
      /** The frame's closing tag never arrived: cut in transit. */
      truncated: boolean;
    }
  | {
      recognized: true;
      kind: "agent_input_required";
      childSessionId: string;
      childName: string | null;
      truncated: boolean;
    }
  | {
      recognized: true;
      kind: "agent_quiet";
      childSessionId: string;
      childName: string | null;
      /** Whole milliseconds; the surface converts to minutes for display. */
      idleMs: number;
      truncated: boolean;
    }
  | {
      recognized: false;
      /** The kind the fixed header declared. A frame without one is not a
          notice and never reaches this type. */
      kind: string;
      /** The frame's universal child pointer, from the fixed header. */
      childSessionId: string | null;
    };

const ENVELOPE_OPEN = "<devboule-system>";
const ENVELOPE_CLOSE = "</devboule-system>";
const DAEMON_ROLE = "daemon";
const KNOWN_KINDS = ["agent_finished", "agent_input_required", "agent_quiet"] as const;
const TIMESTAMP_PREFIX = "timestamp: ";
/** The field lines the finish prose cannot be told apart from. */
const FINISH_MARKERS = ["note: ", "artifacts: "];

function headerValue(line: string, key: string): string | null {
  if (!line.startsWith(`${key}: `)) return null;
  const value = line.slice(key.length + 1).trim();
  return value.length > 0 ? value : null;
}

function headerLinesValue(lines: string[], key: string): string | null {
  for (const line of lines) {
    const value = headerValue(line, key);
    if (value !== null) return value;
  }
  return null;
}

/** The daemon's fixed header: every line before the first timestamp line.
    The daemon composes those lines before any caller-controlled byte; a
    frame without a timestamp line has no provable header at all. */
function headerBlock(lines: string[]): string[] {
  const end = lines.findIndex((line) => line.startsWith(TIMESTAMP_PREFIX));
  return end === -1 ? [] : lines.slice(0, end);
}

function parseFinished(
  lines: string[],
  childSessionId: string,
  truncated: boolean,
): AgentDaemonNotice {
  let childName: string | null = null;
  let state: string | null = null;
  let summary: string | null = null;
  let unattributed: string | null = null;
  let summaryAt = -1;

  // Single-line fields are read only before the summary starts: the child's
  // prose swallows whole lines, and a forged line inside it must not
  // impersonate a header field.
  let at = 0;
  for (; at < lines.length; at += 1) {
    if (lines[at].startsWith("summary:")) {
      summaryAt = at;
      break;
    }
    const name = headerValue(lines[at], "displayName");
    if (name !== null && childName === null) childName = name;
    const stateValue = headerValue(lines[at], "state");
    if (stateValue !== null && state === null) state = stateValue;
  }

  if (summaryAt !== -1 || lines.some((line) => FINISH_MARKERS.some((m) => line.startsWith(m)))) {
    // The daemon's own fields end at the frame's final `artifacts:` line —
    // the last field it writes. Everything from the first marker line to
    // that anchor could be either voice. The scan starts at the summary's
    // first continuation line, or at the body's start when no summary line
    // arrived: a frame can carry the daemon's note with nothing before it.
    const scanStart = summaryAt === -1 ? 0 : summaryAt + 1;
    const tailAnchor =
      lines.length > 0 && lines[lines.length - 1].startsWith("artifacts: ")
        ? lines.length - 1
        : lines.length;
    let markerAt = tailAnchor;
    for (let probe = scanStart; probe < tailAnchor; probe += 1) {
      if (FINISH_MARKERS.some((marker) => lines[probe].startsWith(marker))) {
        markerAt = probe;
        break;
      }
    }
    if (summaryAt !== -1) {
      const first = headerValue(lines[summaryAt], "summary") ?? "";
      const proseLines = lines.slice(summaryAt + 1, markerAt);
      if (markerAt === lines.length && proseLines.at(-1) === "") proseLines.pop();
      const rest = proseLines.join("\n");
      const prose = [first, rest].filter((part) => part.length > 0).join("\n");
      summary = prose.length > 0 ? prose : null;
    }
    if (markerAt < tailAnchor) {
      const tailLines = lines.slice(markerAt, tailAnchor);
      if (tailAnchor === lines.length && tailLines.at(-1) === "") tailLines.pop();
      unattributed = tailLines.length > 0 ? tailLines.join("\n") : null;
    }
  }

  return {
    recognized: true,
    kind: "agent_finished",
    childSessionId,
    childName,
    state,
    summary,
    unattributed,
    truncated,
  };
}

/**
 * Parses one daemon notice out of transcript text. Returns null for
 * everything whose fixed header does not carry BOTH `role: daemon` and a
 * `kind:` line — ordinary messages, permission requests (another parser's
 * frame), and the daemon's agent-to-agent echo, whose payload is another
 * agent's words and whose composed role may itself read `daemon`.
 */
export function parseAgentDaemonNotice(text: string): AgentDaemonNotice | null {
  if (!text.startsWith(ENVELOPE_OPEN)) return null;

  // The frame the daemon composed ends at its first closing tag. A pasted
  // block is not neutralised, so a closer inside carried text cannot grow
  // the frame: content after the closer — let alone a second opener — is
  // not this frame's, and the header block still tells the frame apart.
  const closeIndex = text.indexOf(ENVELOPE_CLOSE);
  const truncated = closeIndex === -1;
  const bodyEnd = truncated ? text.length : closeIndex;
  const foreignTail =
    !truncated && text.indexOf(ENVELOPE_OPEN, closeIndex + ENVELOPE_CLOSE.length) !== -1;

  const lines = text.slice(ENVELOPE_OPEN.length, bodyEnd).split("\n");
  // The frame's own newlines: one right after the open tag, one right before
  // the close tag when it arrived. They are the frame's, not the payload's —
  // the same rule the permission parser applies to its excerpt's end.
  if (lines[0] === "") lines.shift();
  if (lines.at(-1) === "") lines.pop();
  const header = headerBlock(lines);
  if (headerLinesValue(header, "role") !== DAEMON_ROLE) return null;

  // `role:` is not the notice marker. The daemon composes it from the
  // CALLER's peer record (`session.rs:5574`), so an agent-to-agent echo sent
  // by a session a paired daemon created reads `role: daemon` while its body
  // is another agent's words. `kind:` inside the fixed header is the marker:
  // every notice the daemon builds carries one, the echo carries none.
  const kind = headerLinesValue(header, "kind");
  if (kind === null) return null;
  const fromAgent = headerLinesValue(header, "from_agent");
  const unformatted = (): AgentDaemonNotice => ({
    recognized: false,
    kind,
    childSessionId: fromAgent,
  });

  // A recognized kind stands only on the fact its card cannot invent: the
  // child it is about. A frame naming no child is demoted, not half-parsed —
  // the unknown-kind card reports what the frame did declare.
  const childSessionId = headerLinesValue(lines, "childSessionId") ?? fromAgent;
  if (
    foreignTail ||
    kind === null ||
    !KNOWN_KINDS.includes(kind as (typeof KNOWN_KINDS)[number]) ||
    childSessionId === null
  ) {
    return unformatted();
  }

  if (kind === "agent_quiet") {
    const idleValue = headerLinesValue(lines, "idleMs");
    const idleMs = idleValue !== null && /^\d+$/.test(idleValue) ? Number(idleValue) : null;
    if (idleMs === null || !Number.isSafeInteger(idleMs)) {
      return unformatted();
    }
    return {
      recognized: true,
      kind,
      childSessionId,
      childName: headerLinesValue(lines, "displayName"),
      idleMs,
      truncated,
    };
  }

  if (kind === "agent_input_required") {
    return {
      recognized: true,
      kind,
      childSessionId,
      childName: headerLinesValue(lines, "displayName"),
      truncated,
    };
  }

  return parseFinished(lines, childSessionId, truncated);
}
