// The File tab's Preview | Source choice, remembered per app run across
// tabs: module state, because a tab unmounts when it goes inactive and
// the memory must not. Markdown defaults to Preview; a non-Markdown file
// never asks — Source is its only view.

export type FileTabMode = "preview" | "source";

let mode: FileTabMode = "preview";

export function fileTabMode(): FileTabMode {
  return mode;
}

export function setFileTabMode(next: FileTabMode): void {
  mode = next;
}

export function resetFileTabModeForTests(): void {
  mode = "preview";
}
