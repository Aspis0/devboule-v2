/**
 * Whether a tool row is a change to files on disk: the row draws its counts,
 * reveals its file in the Changes panel and prints a path as one.
 *
 * `edit` and `delete` are the kinds providers have always sent. `write` is the
 * spelling a change already carries for a file that was created, and `change`
 * is a set of changes that share no one action. Both are file changes, and
 * neither is an edit — a row wearing one claims the agent changed a file that
 * did not exist.
 */
export function isFileChangeKind(kind: string | undefined): boolean {
  const name = kind?.trim().toLowerCase();
  return name === "edit" || name === "delete" || name === "write" || name === "change";
}
