// What the address bar types becomes: one http(s) URL, or null. This is the
// address bar's own answer; Rust gates every navigation again on the way to
// the webview, so a refusal here is a refusal before the user waits.
//
// Derived from Orca's `src/shared/browser-url.ts`
// (https://github.com/stablyai/orca, MIT, Copyright (c) 2026 Lovecast Inc.):
// the scheme-less classification and the http/https-only gate. Modified — the
// blank page is a real start page rather than `about:blank`, `file:` is
// refused instead of allowed, and the filesystem-path and search-engine
// branches are gone, so a refusal here is a refusal there too.

/** Where a new browser tab starts. A real page, so the tab is never blank. */
export const BROWSER_START_URL = "https://example.com/";

/** A scheme-less loopback or wildcard bind address: these are a developer's
 * own machine, and an https attempt against them only fails. */
const LOCAL_ADDRESS =
  /^(?:localhost|127(?:\.\d{1,3}){3}|0\.0\.0\.0|\[[0-9a-f:]+\])(?::\d+)?(?:[/?#].*)?$/i;

function parse(candidate: string): URL | null {
  try {
    return new URL(candidate);
  } catch {
    return null;
  }
}

/** The only two things this tab will ever load. Checked on the parsed URL,
 * never on the text, so `HTTPS:` and a leading newline land on the same
 * answer as their plain spellings. */
function isWebScheme(parsed: URL): boolean {
  return parsed.protocol === "http:" || parsed.protocol === "https:";
}

/** `example.com:8443` names a host and a port, not the scheme
 * `example.com`. Checked before anything else, because `new URL` accepts that
 * spelling as the nonsense scheme `example.com:` and would refuse it as if
 * the user had typed a scheme on purpose. */
function isDomainAndPort(input: string): boolean {
  const match = /^([^\s/\\:@?#]+):\d+(?:[/?#].*)?$/.exec(input);
  return match !== null && match[1].includes(".");
}

/** Text that already spells its own scheme. Never prefixed: prefixing it
 * would turn `https://` into a host called `https`. */
function namesItsOwnScheme(input: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(input);
}

/** The parsed URL, or null when it is not one this tab loads. */
function webOnly(candidate: string): string | null {
  const parsed = parse(candidate);
  return parsed !== null && isWebScheme(parsed) ? parsed.toString() : null;
}

/**
 * The URL to navigate to, or null when the text is not one this tab loads.
 *
 * Empty input is the start page rather than a refusal: a browser tab that
 * opens on nothing is not a browser tab. A scheme-less host gets https, as a
 * browser does; a loopback or wildcard bind address gets http, where an
 * https attempt only fails.
 */
export function normalizeBrowserUrl(raw: string): string | null {
  const trimmed = raw.trim();
  if (trimmed === "") return BROWSER_START_URL;
  // The loopback test comes before the scheme test, because `localhost:5173`
  // spells a scheme to a regex and to `new URL` both.
  if (LOCAL_ADDRESS.test(trimmed)) return webOnly(`http://${trimmed}`);
  if (isDomainAndPort(trimmed)) return webOnly(`https://${trimmed}`);
  if (namesItsOwnScheme(trimmed)) return webOnly(trimmed);
  return webOnly(`https://${trimmed}`);
}

/**
 * Why the address bar will not navigate, in the words the inline error line
 * shows. The page underneath is left exactly where it was.
 */
export function browserUrlRefusal(raw: string): string | null {
  if (normalizeBrowserUrl(raw) !== null) return null;
  const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(raw.trim());
  if (scheme === null) return "That is not a web address.";
  return `${scheme[1].toLowerCase()}: addresses cannot be opened here.`;
}

/** The chip's name for a page: its own title, else the host it is on. */
export function browserTabLabel(title: string | null, url: string): string {
  const clean = title?.trim();
  if (clean !== undefined && clean !== "") return clean;
  return parse(url)?.hostname || url;
}
