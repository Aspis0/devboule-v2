/**
 * Whether a URL's authority carries a user or a password. One check for every
 * caller that must keep credentials out of the DOM and off the IPC boundary:
 * a URL that carries them is never a link, here or in the command.
 */
export function carriesCredentials(url: string): boolean {
  const schemeEnd = url.indexOf("://");
  // Without an authority there is nowhere for a user or password to sit, so
  // `mailto:someone@example.com` and a plain word carry none.
  if (schemeEnd === -1) return false;
  return url
    .slice(schemeEnd + 3)
    .split(/[/?#]/, 1)[0]
    .includes("@");
}
