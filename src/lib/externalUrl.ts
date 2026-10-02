import { carriesCredentials } from "./urlCredentials";

const MAX_URL_BYTES = 8192;
const CANONICAL_PREFIX = /^https?:\/\//i;
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

function exactUrl(value: string): URL | null {
  if (WHITESPACE.test(value) || hasAsciiControl(value)) return null;
  if (!CANONICAL_PREFIX.test(value) || carriesCredentials(value)) return null;
  try {
    return new URL(value);
  } catch {
    return null;
  }
}

/**
 * Whether the system-browser command opens `value` as sent: the frontend half
 * of its `openable_url`, and the one answer to whether a link is an anchor and
 * whether its click is routed there.
 */
export function opensExternally(value: string): boolean {
  return encoder.encode(value).length <= MAX_URL_BYTES && exactUrl(value) !== null;
}

/**
 * The URL a fetch title links to, or null. The anchor renders and a click sends
 * the normalized href, not the title, so the byte limit applies to the href.
 */
export function linkTarget(title: string): URL | null {
  const url = exactUrl(title);
  return url !== null && opensExternally(url.href) ? url : null;
}
