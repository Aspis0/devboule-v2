import { carriesCredentials } from "./urlCredentials";

const MAX_URL_BYTES = 8192;
// Rust's `char::is_whitespace` is the Unicode White_Space property.
const WHITESPACE = /\p{White_Space}/u;
const encoder = new TextEncoder();

/** Rust's `is_ascii_control`: U+0000–U+001F and U+007F. */
function hasAsciiControl(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x20 || code === 0x7f) return true;
  }
  return false;
}

/**
 * The parsed URL when the system-browser command opens `value`, else null: at
 * most 8192 UTF-8 bytes, no whitespace or ASCII control, a parse, the http or
 * https scheme, and no credentials. It is the frontend half of the command's
 * `openable_url`, and the one answer to whether a link is an anchor and whether
 * its click is routed there.
 */
export function openableUrl(value: string): URL | null {
  if (encoder.encode(value).length > MAX_URL_BYTES) return null;
  if (WHITESPACE.test(value) || hasAsciiControl(value)) return null;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return null;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  if (url.username !== "" || url.password !== "" || carriesCredentials(value)) return null;
  return url;
}

export function opensExternally(value: string): boolean {
  return openableUrl(value) !== null;
}
