/**
 * Whether the raw authority of a URL, the text between `//` and the first `/`,
 * `?` or `#`, holds an `@`. A parser drops an empty user and password, so only
 * the raw text tells `https://@host/` from `https://host/`.
 */
export function carriesCredentials(url: string): boolean {
  const authorityStart = url.indexOf("//");
  if (authorityStart === -1) return false;
  return url
    .slice(authorityStart + 2)
    .split(/[/?#]/, 1)[0]
    .includes("@");
}
