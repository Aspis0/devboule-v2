const AUTHORITY = /^https?:\/\/([^/\\?#]*)/i;

/**
 * Whether an `http://` or `https://` URL's authority, the text after the scheme
 * up to the first `/`, `\`, `?` or `#`, holds an `@`. A parser drops an empty
 * user and password, so only the raw text tells `https://@host/` from
 * `https://host/`; an `@` later in the URL is not userinfo.
 */
export function carriesCredentials(url: string): boolean {
  return AUTHORITY.exec(url)?.[1]?.includes("@") ?? false;
}
