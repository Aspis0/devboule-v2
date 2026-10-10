/**
 * The "@" file tag in the composer: the token being typed at the end of the
 * draft, the workspace files it matches, and the text it becomes once a file
 * is picked. The file travels as its workspace-relative path in the prompt,
 * and the agent opens it with its own tools — the same as Paseo's mention.
 */

/** Most files the "@" menu offers at once. */
export const FILE_MENTION_LIMIT = 8;

/**
 * The query of the "@" token at the end of the draft, or `null` when the draft
 * does not end inside one. The "@" must start the draft or follow whitespace,
 * so an address in the middle of a word is not a tag.
 */
export function fileMentionQuery(input: string): string | null {
  const match = /(?:^|\s)@([^\s@]*)$/.exec(input);
  return match === null ? null : (match[1] ?? "");
}

/**
 * The files the menu offers for `query`, best first: a file whose own name
 * starts with the query, then one whose path does, then any that contains it.
 * Ties keep the listing's order, shorter paths first.
 */
export function rankFileMatches(paths: readonly string[], query: string): string[] {
  const needle = query.toLowerCase();
  if (needle === "") return paths.slice(0, FILE_MENTION_LIMIT);
  const scored: Array<{ path: string; rank: number }> = [];
  for (const path of paths) {
    const lower = path.toLowerCase();
    const name = lower.slice(lower.lastIndexOf("/") + 1);
    let rank: number;
    if (name.startsWith(needle)) rank = 0;
    else if (lower.startsWith(needle)) rank = 1;
    else if (name.includes(needle)) rank = 2;
    else if (lower.includes(needle)) rank = 3;
    else continue;
    scored.push({ path, rank });
  }
  scored.sort((left, right) => left.rank - right.rank || left.path.length - right.path.length);
  return scored.slice(0, FILE_MENTION_LIMIT).map((entry) => entry.path);
}

/** The draft with its trailing "@" token replaced by the picked path. */
export function insertFileMention(input: string, path: string): string {
  return input.replace(/@[^\s@]*$/, `@${path} `);
}
