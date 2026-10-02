import type { Id } from "../types/ipc";
import { unsafeCharacterName } from "./sessionRename";

/** The same cap a session display name has: both render as a row's label
 * (`MAX_DISPLAY_NAME_CHARS`). */
export const WORKSPACE_TITLE_MAX_CHARS = 60;

/** Mirrors `validate_workspace_title` (devboule-protocol) sentence for
 * sentence, so the row's door and the daemon's door refuse the same names. */
export function validateWorkspaceTitle(title: string): string | null {
  const trimmed = title.trim();
  if (trimmed === "") return "A workspace title is required; it was empty.";
  const category = unsafeCharacterName(trimmed);
  if (category !== null) return `A workspace title must not contain ${category}.`;
  const length = [...trimmed].length;
  if (length > WORKSPACE_TITLE_MAX_CHARS) {
    return `A workspace title is ${length} characters; the limit is ${WORKSPACE_TITLE_MAX_CHARS}.`;
  }
  return null;
}

/** Display only — stored titles, ids and sessions are untouched; numbered
 * over the full list so a row's label never moves with the search query. */
export function workspaceDisplayTitles(
  workspaces: readonly { id: Id; title: string }[],
): ReadonlyMap<Id, string> {
  // Every stored title is held by the row that keeps it (the first with that
  // title), so a numbered candidate may not reuse any of them.
  const held = new Set(workspaces.map((workspace) => workspace.title));
  const seen = new Map<string, number>();
  const titles = new Map<Id, string>();
  for (const workspace of workspaces) {
    const nth = (seen.get(workspace.title) ?? 0) + 1;
    seen.set(workspace.title, nth);
    if (nth === 1) {
      titles.set(workspace.id, workspace.title);
      continue;
    }
    let number = nth;
    let candidate = `${workspace.title} ${number}`;
    while (held.has(candidate)) {
      number += 1;
      candidate = `${workspace.title} ${number}`;
    }
    held.add(candidate);
    titles.set(workspace.id, candidate);
  }
  return titles;
}
