// The daemon's session-name rule, mirrored on the client so a name the
// daemon would refuse is refused here first — with the daemon's own
// sentences, so the two doors read alike. The rule itself is
// `validate_display_name` (devboule-protocol/src/messages.rs:961): trim,
// then refuse empty, unsafe characters, and over 60. The daemon stores the
// trimmed value, so the trimmed value is the one judged and the one sent.

/** The handshake capability that gates `SessionSetName`: the daemon refuses
 * the frame with capability_not_supported when the name was not negotiated
 * (server/dispatch.rs:320, the session-RPC arm). */
export const SESSION_RENAME_CAPABILITY = "sessions";

/** The daemon's session-name limit, in characters
 * (devboule-protocol/src/lib.rs:381). */
export const SESSION_DISPLAY_NAME_MAX_CHARS = 60;

/** The seven scalars that render as a new line even where `\n` is absent
 * (devboule-protocol/src/text_safety.rs:9). */
const MANDATORY_LINE_BREAKS = new Set([
  "\r",
  "\n",
  "\u{b}",
  "\u{c}",
  "\u{85}",
  "\u{2028}",
  "\u{2029}",
]);

function isInvisibleFormat(character: string): boolean {
  const code = character.codePointAt(0) ?? 0;
  return (
    code === 0x00ad ||
    code === 0x061c ||
    (code >= 0x200b && code <= 0x200f) ||
    (code >= 0x202a && code <= 0x202e) ||
    (code >= 0x2060 && code <= 0x2064) ||
    (code >= 0x2066 && code <= 0x2069) ||
    code === 0xfeff
  );
}

/** The first character category that must never ride a display name, named
 * for a caller to put in its own sentence — `None` when every character is
 * plain. Mirrors `unsafe_character` (text_safety.rs:34): control, invisible
 * formatting, then a mandatory line break. */
export function unsafeCharacterName(name: string): string | null {
  for (const character of name) {
    if (/\p{Cc}/u.test(character)) return "a control character";
    if (isInvisibleFormat(character)) return "an invisible formatting character";
    if (MANDATORY_LINE_BREAKS.has(character)) return "a line break character";
  }
  return null;
}

/** The refusal sentence for a name that must not be sent, or null when the
 * name may be sent. The daemon trims before storing, so the trimmed value
 * is what the caller judges and sends; the sentences are the daemon's own
 * (messages.rs:961), so a client refusal and a daemon refusal read the same. */
export function validateSessionRename(name: string): string | null {
  const trimmed = name.trim();
  if (trimmed === "") return "A session display name is required; it was empty.";
  const category = unsafeCharacterName(trimmed);
  if (category !== null) return `A session display name must not contain ${category}.`;
  const length = [...trimmed].length;
  if (length > SESSION_DISPLAY_NAME_MAX_CHARS) {
    return `A session display name is ${length} characters; the limit is ${SESSION_DISPLAY_NAME_MAX_CHARS}.`;
  }
  return null;
}
