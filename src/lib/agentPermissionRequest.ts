/**
 * The parser for the one `<devboule-system>` envelope this app reads out of a
 * transcript text: `kind: agent_permission_request`, delivered to a creator's
 * transcript through the daemon's send path, so the app sees it as an echoed
 * user message.
 *
 * The finish envelope has a structured event beside it and the app must not
 * grow a parser for THAT frame (`child_finished` in `types/ipc.ts`). This one
 * has no structured twin on the wire — the excerpt is text the child chose,
 * and the whole point is that it reaches the creator's prompt as words — so
 * the app reads the frame here, once, and keeps what it parses in one chat
 * item type.
 *
 * The grammar this side commits to (the daemon half is checked against it):
 *
 *     <devboule-system>
 *     origin: …
 *     role: daemon
 *     from_agent: …
 *     kind: agent_permission_request
 *     timestamp: …
 *     cardId: …            \  the daemon's own facts — single
 *     toolTitle: …          \  lines, rendered in system
 *     displayName: …       /   styling by the chat surface
 *     child-said:
 *     …the child's own words, any number of lines…
 *     end child-said
 *     </devboule-system>
 *
 * - The excerpt block opens at a line that is exactly `child-said:` and
 *   closes at a line that is exactly `end child-said` — EXACT, never
 *   trimmed: the daemon neutralises the exact literal inside the excerpt, so
 *   a padded or tabbed fence line is the child's own text, and honouring it
 *   as a delimiter would let the child choose where its quoted words stop.
 *   If the closer never arrives the block runs to the envelope's closing tag
 *   rather than being dropped, and the surface says the fence never closed.
 * - The unit contract for the excerpt's 512 cap is the DAEMON's, one place:
 *   512 Unicode scalar values counted on the raw text before escaping, cut at
 *   a scalar boundary. The escaped wire form may exceed 512 units and this
 *   side NEVER re-truncates — it renders what it was sent, verbatim, through
 *   React's default text escaping.
 * - Fields the daemon may add later are ignored, so a newer daemon cannot
 *   break this one.
 *
 * Naming honesty, per the design: the quoting of the excerpt is a
 * **mitigation, not a fix**. A confused or hostile child can still write
 * "ignore your instructions" into the excerpt; the quoted block keeps the
 * creator's model — and the human reading over its shoulder — able to tell
 * whose words they are, and nothing more.
 */

export interface AgentPermissionRequestFields {
  cardId: string;
  toolTitle: string;
  childName: string;
  excerpt: string;
  /**
   * Whether the excerpt's closing fence arrived. `"absent"` — no
   * `child-said:` opener, so there is nothing quoted; `"closed"` — the exact
   * closer was found; `"unterminated"` — the opener arrived but the closer
   * never did, so the block runs to the envelope's end. The surface renders
   * the unterminated state: a frame the daemon's own contract broke is a fact
   * the human reads, not one the app absorbs silently.
   */
  excerptState: "closed" | "unterminated" | "absent";
}

const ENVELOPE_OPEN = "<devboule-system>";
const ENVELOPE_CLOSE = "</devboule-system>";
const KIND_LINE = "kind: agent_permission_request";
const EXCERPT_OPEN = "child-said:";
const EXCERPT_CLOSE = "end child-said";

function headerValue(line: string, key: string): string | null {
  if (!line.startsWith(`${key}: `)) return null;
  const value = line.slice(key.length + 1).trim();
  return value.length > 0 ? value : null;
}

/**
 * Parses one permission-request envelope out of transcript text, or returns
 * null for everything else — every ordinary user message, every other
 * envelope kind, and any malformed frame, which renders as the raw text it
 * arrived as rather than being half-interpreted.
 */
export function parseAgentPermissionRequest(text: string): AgentPermissionRequestFields | null {
  if (!text.startsWith(ENVELOPE_OPEN)) return null;
  const closeIndex = text.lastIndexOf(ENVELOPE_CLOSE);
  if (closeIndex === -1) return null;
  const body = text.slice(ENVELOPE_OPEN.length, closeIndex);
  const lines = body.split("\n");

  let cardId: string | null = null;
  let toolTitle: string | null = null;
  let childName: string | null = null;
  let kindMatched = false;
  let excerptStart = -1;

  for (let at = 0; at < lines.length; at += 1) {
    const line = lines[at];
    if (!kindMatched) {
      if (line.trim() === KIND_LINE) kindMatched = true;
      continue;
    }
    // EXACT line matches only — never trimmed. The daemon neutralises the
    // exact literal inside the excerpt; a padded `end child-said` is a line
    // the escaper's contract leaves alone, so it is the child's own words,
    // and honouring it as a closer would let the child choose where its
    // quoted words stop and hide everything after that line from the human.
    if (line === EXCERPT_OPEN) {
      excerptStart = at + 1;
      break;
    }
    const card = headerValue(line, "cardId");
    if (card !== null) {
      cardId = card;
      continue;
    }
    const title = headerValue(line, "toolTitle");
    if (title !== null) {
      toolTitle = title;
      continue;
    }
    const name = headerValue(line, "displayName");
    if (name !== null) childName = name;
  }

  if (!kindMatched || cardId === null || toolTitle === null || childName === null) {
    return null;
  }

  // The excerpt runs to its exact closer, or to the end of the body when the
  // closer never came: dropping a child's words for a missing fence would
  // hide the one text the human most needs to see marked as foreign.
  let excerpt: string;
  let excerptState: AgentPermissionRequestFields["excerptState"];
  if (excerptStart === -1) {
    excerpt = "";
    excerptState = "absent";
  } else {
    let end = lines.length;
    for (let at = excerptStart; at < lines.length; at += 1) {
      if (lines[at] === EXCERPT_CLOSE) {
        end = at;
        break;
      }
    }
    excerptState = end === lines.length ? "unterminated" : "closed";
    if (excerptState === "closed") {
      // The grammar has exactly one quoted block — the daemon composes a
      // single `child-said:` opener line. An opener AFTER the closer means a
      // header value swallowed a newline and planted a complete second block
      // (the header's fields are documented single daemon-composed lines; a
      // `displayName` carrying `\nchild-said:\nend child-said` is re-audit
      // F14, and its quoted block then ends where a header value said it
      // did, dropping the real one silently). That frame is malformed, and a
      // malformed frame renders as the raw text it arrived as rather than
      // being half-interpreted. A bare extra closer is deliberately NOT this
      // rule: the block keeps the words before it, and preventing that is
      // the daemon escaper's job, not a guess this side can make.
      for (let at = end + 1; at < lines.length; at += 1) {
        if (lines[at] === EXCERPT_OPEN) return null;
      }
    }
    // Without the closer the block runs to the envelope body's end; the
    // body's own trailing newline before the closing tag is the frame's, not
    // the child's, so it does not join the excerpt.
    const collected = lines.slice(excerptStart, end);
    if (end === lines.length && collected.at(-1) === "") collected.pop();
    excerpt = collected.join("\n");
  }

  return { cardId, toolTitle, childName, excerpt, excerptState };
}
