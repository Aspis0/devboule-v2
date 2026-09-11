import type { SectionNote } from "./designHost";

/**
 * Anchored agent notes: one short instruction per page section, resolved by
 * the section's stable anchor and injected into the next generation's scope
 * block. Kept deliberately minimal — a field, a list, a mark — not a thread
 * system.
 */

/** Longest note text kept; longer input is trimmed, never silently kept whole. */
export const MAX_SECTION_NOTE_CHARS = 2000;
/** Most notes kept per document; the list is a scratch margin, not storage. */
export const MAX_SECTION_NOTES = 200;
export const MAX_SECTION_NOTE_ANCHOR_CHARS = 220;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Drop malformed entries from a loaded document; a bad note must not break load. */
export function sanitizeSectionNotes(value: unknown): SectionNote[] {
  if (!Array.isArray(value)) return [];
  const notes: SectionNote[] = [];
  for (const entry of value) {
    if (!isRecord(entry)) continue;
    const anchor = entry.anchor;
    const text = entry.text;
    if (
      typeof anchor !== "string" ||
      anchor.length === 0 ||
      anchor.length > MAX_SECTION_NOTE_ANCHOR_CHARS ||
      typeof text !== "string" ||
      text.trim().length === 0
    ) {
      continue;
    }
    notes.push({ anchor, text: text.trim().slice(0, MAX_SECTION_NOTE_CHARS) });
    if (notes.length >= MAX_SECTION_NOTES) break;
  }
  return notes;
}

export function cloneSectionNotes(notes: readonly SectionNote[]): SectionNote[] {
  return notes.map((note) => ({ ...note }));
}

export interface ResolvedSectionNote {
  note: SectionNote;
  /** Index in the document's note list, for deletion. */
  index: number;
}

export interface ResolvedSectionNotes {
  /** Notes whose anchor resolves against the current page. */
  matched: ResolvedSectionNote[];
  /**
   * Notes whose anchor is gone from the current page. Shown with an explicit
   * orphan badge and still sent to the agent flagged — never dropped silently.
   * Empty when there is no artifact to resolve against: with nothing measured,
   * "unresolvable" says nothing.
   */
  orphans: ResolvedSectionNote[];
}

/**
 * Split notes into resolved and orphaned against the current page's anchors.
 * Order is stable: document order within each group.
 */
export function resolveSectionNotes(
  notes: readonly SectionNote[],
  knownAnchors: ReadonlySet<string>,
  artifactPresent: boolean,
): ResolvedSectionNotes {
  const matched: ResolvedSectionNote[] = [];
  const orphans: ResolvedSectionNote[] = [];
  notes.forEach((note, index) => {
    const entry = { note, index };
    if (!artifactPresent || knownAnchors.has(note.anchor)) matched.push(entry);
    else orphans.push(entry);
  });
  return { matched, orphans };
}

/**
 * The notes block appended to the generation scope, in the same place the
 * `Scope:` line goes. Only the selected section's notes travel with a
 * generation — every section's notes on every run would be noise — plus any
 * orphans, flagged as detached, so a rewritten page can never lose a note
 * silently. Empty string when there is nothing to say.
 */
export function formatSectionNotesScope(
  selectedAnchor: string | null,
  notes: readonly SectionNote[],
  knownAnchors: ReadonlySet<string>,
  artifactPresent: boolean,
): string {
  const { orphans } = resolveSectionNotes(notes, knownAnchors, artifactPresent);
  const lines: string[] = [];
  if (selectedAnchor !== null) {
    for (const note of notes) {
      if (note.anchor === selectedAnchor) lines.push(`- [${note.anchor}]: ${note.text}`);
    }
  }
  const blocks: string[] = [];
  if (lines.length > 0) blocks.push(`Section notes:\n${lines.join("\n")}`);
  if (orphans.length > 0) {
    const orphanLines = orphans.map(
      (entry) =>
        `- [${entry.note.anchor}] (anchor not found in the current page): ${entry.note.text}`,
    );
    blocks.push(
      `Detached notes (their anchors were not found in the current page — kept, not dropped):\n${orphanLines.join("\n")}`,
    );
  }
  return blocks.join("\n");
}
