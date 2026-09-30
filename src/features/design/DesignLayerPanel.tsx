import { Fragment, memo, useEffect, useRef, useState } from "react";
import type { RefObject } from "react";
import { isImeComposition } from "../../lib/imeComposition";
import type { DesignLayer } from "./designHost";
import type { ResolvedSectionNote } from "./sectionNotes";

export interface LayerViewModel extends DesignLayer {
  selected: boolean;
  hidden: boolean;
  /** A section layer carries at least one agent note. Canvas nodes never do. */
  hasNote: boolean;
}

/** One clickable step of the root-to-leaf chain shown for the selected layer. */
export interface LayerChainStep {
  id: string;
  name: string;
}

interface LayerPanelProps {
  /**
   * Top-level layers only: canvas nodes and page-section roots, in document
   * order. The deep sections are discovered by clicking the canvas, so the
   * navigator stays as short as the page is at its root.
   */
  navigator: readonly LayerViewModel[];
  /** The selected layer, or null; its details render in the panel. */
  selected: LayerViewModel | null;
  /** Root-to-leaf chain for the selected section, inclusive. */
  ancestors: readonly LayerChainStep[];
  onSelect: (layerId: string) => void;
  onDeselect: () => void;
  onToggleVisibility: (layerId: string) => void;
  /** Notes whose anchor is gone from the current page; shown, not dropped. */
  orphanNotes: readonly ResolvedSectionNote[];
  onDeleteNote: (index: number) => void;
  /** Notes on the selected section; empty unless a section is selected. */
  selectedSectionNotes: readonly ResolvedSectionNote[];
  onAddNote: (text: string) => void;
}

interface LayerRowProps {
  layer: LayerViewModel;
  /** Renders the row as the selected row and appends its details. */
  expanded: boolean;
  /** Set on the one expanded row, so the panel can reveal it after layout. */
  rowRef?: RefObject<HTMLDivElement | null>;
  ancestors: readonly LayerChainStep[];
  onSelect: (layerId: string) => void;
  onDeselect: () => void;
  onToggleVisibility: (layerId: string) => void;
  /** Notes on the selected section; only the expanded section row reads them. */
  selectedSectionNotes: readonly ResolvedSectionNote[];
  onAddNote: (text: string) => void;
  onDeleteNote: (index: number) => void;
}

interface SectionDetailsProps {
  layer: LayerViewModel;
  /** Root-to-leaf chain for the selected section, inclusive. */
  ancestors: readonly LayerChainStep[];
  notes: readonly ResolvedSectionNote[];
  onSelectAncestor: (layerId: string) => void;
  onAddNote: (text: string) => void;
  onDeleteNote: (index: number) => void;
  onDeselect: () => void;
}

const SectionDetails = memo(function SectionDetails({
  layer,
  ancestors,
  notes,
  onSelectAncestor,
  onAddNote,
  onDeleteNote,
  onDeselect,
}: SectionDetailsProps) {
  const [noteDraft, setNoteDraft] = useState("");
  const section = layer.section;
  if (section === undefined) return null;
  const submitNote = () => {
    const text = noteDraft.trim();
    if (text.length === 0) return;
    onAddNote(text);
    setNoteDraft("");
  };
  return (
    <div className="design-layer-details">
      <div className="design-layer-diagnostics">
        <span className="design-mono-value design-layer-measured">
          {Math.round(layer.transform.width)} × {Math.round(layer.transform.height)} px
        </span>
        <button
          className="design-layer-details-close"
          type="button"
          aria-label="Deselect section"
          onClick={onDeselect}
        >
          ×
        </button>
        <span className="design-mono-value design-layer-anchor">{section.anchor}</span>
      </div>
      {/*
        Root-to-leaf chain: the answer to "I clicked the phrase but meant the
        whole slide". One step is the layer itself, which says nothing, so the
        trail appears only when there is somewhere to climb.
      */}
      {ancestors.length > 1 ? (
        <nav className="design-layer-trail" aria-label="Layer ancestry">
          {ancestors.map((step, index) => (
            <Fragment key={step.id}>
              {index > 0 ? (
                <span className="design-layer-trail-sep" aria-hidden="true">
                  ›
                </span>
              ) : null}
              <button
                type="button"
                className="design-layer-trail-step"
                aria-current={index === ancestors.length - 1 ? "true" : undefined}
                onClick={() => onSelectAncestor(step.id)}
              >
                {step.name}
              </button>
            </Fragment>
          ))}
        </nav>
      ) : null}
      {notes.length > 0 ? (
        <ul className="design-section-notes">
          {notes.map((entry) => (
            <li key={entry.index}>
              <span className="design-section-note-text">{entry.note.text}</span>
              <button
                type="button"
                aria-label="Delete note"
                onClick={() => onDeleteNote(entry.index)}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      ) : null}
      <div className="design-section-note-compose">
        <input
          type="text"
          value={noteDraft}
          maxLength={2000}
          placeholder="Note for the agent on this section…"
          aria-label="Note for the agent on this section"
          onChange={(event) => setNoteDraft(event.target.value)}
          onKeyDown={(event) => {
            if (isImeComposition(event.nativeEvent)) return;
            if (event.key === "Enter") {
              event.preventDefault();
              submitNote();
            } else if (event.key === "Escape" && noteDraft.length > 0) {
              // The note field owns the first Escape: clear the draft instead
              // of deselecting the section. stopPropagation keeps the surface's
              // global Escape (deselect) off this keypress; an empty field lets
              // it through, so deselecting from here still works.
              event.preventDefault();
              event.stopPropagation();
              setNoteDraft("");
            }
          }}
        />
        <button type="button" onClick={submitNote} disabled={noteDraft.trim().length === 0}>
          Add
        </button>
      </div>
    </div>
  );
});

/**
 * Scroll position that reveals a target inside a scroller, or the current one
 * when the target is already fully visible. Positions are in the scroller's own
 * coordinate space; the caller measures them, so the geometry stays pure and
 * testable. The bottom is checked first: an expanded row grows downward, and the
 * content just revealed is what must come into view.
 */
export function revealScrollTopFor(
  scrollTop: number,
  clientHeight: number,
  targetTop: number,
  targetHeight: number,
): number {
  const bottom = targetTop + targetHeight;
  if (bottom > scrollTop + clientHeight) return Math.max(0, bottom - clientHeight);
  if (targetTop < scrollTop) return Math.max(0, targetTop);
  return scrollTop;
}

/**
 * One row of the navigator or of the selected-layer inspector. The expanded
 * row is the only one that opens the note collector, so a long index never
 * paints more than the one layer the user is working on.
 */
const LayerRow = memo(function LayerRow({
  layer,
  expanded,
  rowRef,
  ancestors,
  onSelect,
  onDeselect,
  onToggleVisibility,
  selectedSectionNotes,
  onAddNote,
  onDeleteNote,
}: LayerRowProps) {
  return (
    <div className={`design-layer-row${expanded ? " design-layer-row-selected" : ""}`} ref={rowRef}>
      <button
        className="design-layer-select"
        type="button"
        aria-pressed={layer.selected}
        aria-label={`Select ${layer.name}`}
        onClick={() => onSelect(layer.id)}
      >
        <span className="design-layer-kind">{layer.section?.tag ?? layer.kind}</span>
        <span className={`design-layer-name${layer.hidden ? " design-layer-name-hidden" : ""}`}>
          {layer.name}
        </span>
        {layer.hasNote ? (
          <span
            className="design-layer-note-dot"
            title="Has an agent note"
            aria-label="Has an agent note"
          />
        ) : null}
      </button>
      <button
        className="design-layer-visibility"
        type="button"
        aria-pressed={!layer.hidden}
        aria-label={`${layer.hidden ? "Show" : "Hide"} ${layer.name}`}
        title="Hide / show"
        onClick={() => onToggleVisibility(layer.id)}
      >
        {layer.hidden ? "◌" : "◉"}
      </button>
      {expanded && layer.section !== undefined ? (
        <SectionDetails
          layer={layer}
          ancestors={ancestors}
          notes={selectedSectionNotes}
          onSelectAncestor={onSelect}
          onAddNote={onAddNote}
          onDeleteNote={onDeleteNote}
          onDeselect={onDeselect}
        />
      ) : null}
    </div>
  );
});

export const LayerPanel = memo(function LayerPanel({
  navigator,
  selected,
  ancestors,
  onSelect,
  onDeselect,
  onToggleVisibility,
  orphanNotes,
  onDeleteNote,
  selectedSectionNotes,
  onAddNote,
}: LayerPanelProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const expandedRowRef = useRef<HTMLDivElement>(null);
  const expandedRowId = selected?.id ?? null;
  const expandedRowNoteCount = selected?.section === undefined ? 0 : selectedSectionNotes.length;
  // A navigator of one row is noise: with a single root there is nothing to
  // jump between, and the layer is discovered by clicking it on the canvas. The
  // selected row still renders, so the collector is never lost.
  const showNavigator = navigator.length > 1;
  // When the selected layer already owns a navigator row, its details expand
  // there instead of painting the same layer twice.
  const selectedInNavigator =
    showNavigator && selected !== null && navigator.some((row) => row.id === selected.id);
  const inspectorRow = selected !== null && !selectedInNavigator ? selected : null;

  // Selecting a section expands its row inside the scroller; the revealed note
  // and diagnostics must not stay cut off below the panel. Measured here, after
  // layout, and only when the disclosure changes, so a manual scroll of a list
  // whose selection did not move is never fought.
  useEffect(() => {
    if (expandedRowId === null) return;
    const list = listRef.current;
    const row = expandedRowRef.current;
    if (list === null || row === null) return;
    const listRect = list.getBoundingClientRect();
    const rowRect = row.getBoundingClientRect();
    const targetTop = rowRect.top - listRect.top + list.scrollTop;
    const next = revealScrollTopFor(list.scrollTop, list.clientHeight, targetTop, rowRect.height);
    if (next !== list.scrollTop) list.scrollTop = next;
  }, [expandedRowId, expandedRowNoteCount]);

  return (
    <section className="design-layers-panel" aria-labelledby="design-layers-title">
      <div className="design-overlay-heading">
        <span id="design-layers-title">Layers</span>
        <span className="design-layer-count">{navigator.length}</span>
      </div>
      <div className="design-layer-list" ref={listRef}>
        {showNavigator
          ? navigator.map((layer) => (
              <LayerRow
                key={layer.id}
                layer={layer}
                expanded={expandedRowId === layer.id}
                rowRef={expandedRowId === layer.id ? expandedRowRef : undefined}
                ancestors={ancestors}
                onSelect={onSelect}
                onDeselect={onDeselect}
                onToggleVisibility={onToggleVisibility}
                selectedSectionNotes={selectedSectionNotes}
                onAddNote={onAddNote}
                onDeleteNote={onDeleteNote}
              />
            ))
          : null}
        {inspectorRow !== null ? (
          <LayerRow
            key={inspectorRow.id}
            layer={inspectorRow}
            expanded
            rowRef={expandedRowRef}
            ancestors={ancestors}
            onSelect={onSelect}
            onDeselect={onDeselect}
            onToggleVisibility={onToggleVisibility}
            selectedSectionNotes={selectedSectionNotes}
            onAddNote={onAddNote}
            onDeleteNote={onDeleteNote}
          />
        ) : null}
        {/*
          Detached notes scroll with the rest. As a `flex: none` sibling of the
          scroller they could not shrink, so a tall selection could only push
          them into the panel's `overflow: hidden` clip, where their delete
          control is unreachable. Inside the scroller every note scrolls back
          into view.
        */}
        {orphanNotes.length > 0 ? (
          <div className="design-layer-orphans">
            <div className="design-layer-orphans-heading">
              Detached notes ({orphanNotes.length})
            </div>
            <ul className="design-layer-orphans-list">
              {orphanNotes.map((entry) => (
                <li key={`${entry.note.anchor}:${entry.index}`}>
                  <span className="design-layer-orphan-badge">orphan</span>
                  <span className="design-layer-orphan-anchor">{entry.note.anchor}</span>
                  <span className="design-layer-orphan-text">{entry.note.text}</span>
                  <button
                    type="button"
                    aria-label={`Delete detached note on ${entry.note.anchor}`}
                    onClick={() => onDeleteNote(entry.index)}
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
          </div>
        ) : null}
      </div>
    </section>
  );
});
