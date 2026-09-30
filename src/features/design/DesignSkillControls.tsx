import { memo, useCallback, useEffect, useRef, useState } from "react";
import { isImeComposition } from "../../lib/imeComposition";
import { useModalOpen } from "../../lib/modalOpen";
import { MAX_AUTOMATIC_SKILL_SECTIONS, type BuiltInSkillIndexEntry } from "./builtInSkills";
import { SKILL_MODE_LABELS, type DesignSkillSelection } from "./designSettings";
import { buildSkillBlock } from "./skillLoader";

export interface DesignSkillViewProps {
  skillIndex: readonly BuiltInSkillIndexEntry[];
  skillSelection: DesignSkillSelection;
  selectedSkillSlugs: readonly string[];
  resolvedSkillSlugs: readonly string[] | null;
  appliedSkillSlugs: readonly string[] | null;
  hasResolvedComposition: boolean;
  skillBlock: ReturnType<typeof buildSkillBlock>;
  resolvedSkillSlugSet: ReadonlySet<string>;
  automaticBaselineSlugSet: ReadonlySet<string>;
  droppedSkillSlugSet: ReadonlySet<string>;
}

interface DesignCraftSheetProps extends DesignSkillViewProps {
  /** The sheet is up; the parent owns the open state and says so. */
  open: boolean;
  readOnly: boolean;
  onClose: () => void;
  onSkillToggle: (slug: string) => void;
}

const DESIGN_SKILL_MODES: readonly DesignSkillSelection["mode"][] = ["all", "manual", "auto"];

function renderCraftInline(text: string) {
  return text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g).map((part, index) => {
    if (part.startsWith("**") && part.endsWith("**")) {
      return <strong key={index}>{part.slice(2, -2)}</strong>;
    }
    if (part.startsWith("`") && part.endsWith("`")) {
      return <code key={index}>{part.slice(1, -1)}</code>;
    }
    return part;
  });
}

function renderCraftBody(body: string) {
  return body
    .split(/\n\s*\n/)
    .map((paragraph, index) => (
      <p key={index}>{renderCraftInline(paragraph.replace(/\n/g, " "))}</p>
    ));
}

// Exported for the modal-contract walking test in src/app/modals-over-crescent.test.tsx.
export const DesignSkillModeControl = memo(function DesignSkillModeControl({
  skillSelection,
  onSkillModeChange,
  onCraftOpen,
  onCraftReadMore,
}: {
  skillSelection: DesignSkillSelection;
  onSkillModeChange: (mode: DesignSkillSelection["mode"]) => void;
  onCraftOpen: () => void;
  onCraftReadMore: () => void;
}) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const selectedModeRef = useRef<HTMLButtonElement>(null);
  const activeCopy = SKILL_MODE_LABELS[skillSelection.mode];

  useModalOpen(open);

  const closePopover = useCallback(() => {
    setOpen(false);
    queueMicrotask(() => triggerRef.current?.focus());
  }, []);

  useEffect(() => {
    if (!open) return;
    selectedModeRef.current?.focus();

    const handleKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (isImeComposition(event)) return;
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      closePopover();
    };
    const handlePointerDown = (event: PointerEvent): void => {
      const target = event.target;
      if (
        target instanceof Node &&
        !popoverRef.current?.contains(target) &&
        !triggerRef.current?.contains(target)
      ) {
        closePopover();
      }
    };

    document.addEventListener("keydown", handleKeyDown);
    document.addEventListener("pointerdown", handlePointerDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
      document.removeEventListener("pointerdown", handlePointerDown);
    };
  }, [closePopover, open]);

  const chooseMode = useCallback(
    (mode: DesignSkillSelection["mode"]) => {
      if (skillSelection.mode === mode) return;
      onSkillModeChange(mode);
      closePopover();
      if (mode === "manual") onCraftOpen();
    },
    [closePopover, onCraftOpen, onSkillModeChange, skillSelection.mode],
  );

  const openCraft = useCallback(() => {
    closePopover();
    if (skillSelection.mode === "manual") onCraftOpen();
    else onCraftReadMore();
  }, [closePopover, onCraftOpen, onCraftReadMore, skillSelection.mode]);

  return (
    <div className="design-skill-controls">
      <fieldset className="design-skill-mode-fieldset">
        <legend className="design-sr-only">Craft mode</legend>
        <button
          ref={triggerRef}
          className="design-skill-mode-control"
          type="button"
          data-design-skill-mode-trigger="true"
          aria-label={`Craft mode: ${activeCopy.name} · ${activeCopy.summary ?? activeCopy.blurb}`}
          aria-haspopup="dialog"
          aria-expanded={open}
          aria-controls="design-skill-picker"
          onClick={() => setOpen((current) => !current)}
        >
          <span className="design-skill-mode-control-name">{activeCopy.name}</span>
          <span className="design-skill-mode-control-chevron" aria-hidden="true">
            ⌄
          </span>
        </button>
      </fieldset>
      {open ? (
        <div
          ref={popoverRef}
          id="design-skill-picker"
          className="design-agent-picker design-skill-picker"
          role="dialog"
          aria-labelledby="design-skill-picker-title"
          tabIndex={-1}
        >
          <div className="design-agent-picker-label" id="design-skill-picker-title">
            Craft mode
          </div>
          <p className="design-skill-picker-default">
            <strong>Default:</strong> {SKILL_MODE_LABELS.all.defaultNotice}
          </p>
          <div className="design-skill-mode-options" role="radiogroup" aria-label="Craft mode">
            {DESIGN_SKILL_MODES.map((mode) => {
              const copy = SKILL_MODE_LABELS[mode];
              const selected = skillSelection.mode === mode;
              return (
                <button
                  ref={selected ? selectedModeRef : undefined}
                  className={[
                    "design-skill-mode-option",
                    `design-skill-mode-option-${mode}`,
                    mode === "all" ? "design-skill-mode-option-default" : null,
                  ]
                    .filter((className): className is string => className !== null)
                    .join(" ")}
                  key={mode}
                  type="button"
                  role="radio"
                  data-design-skill-mode={mode}
                  aria-checked={selected}
                  onClick={() => chooseMode(mode)}
                >
                  <span className="design-skill-mode-option-name">
                    {copy.name}
                    <span className="design-skill-mode-option-badge">{copy.badge}</span>
                    {selected ? (
                      <span className="design-skill-mode-option-selected">Selected</span>
                    ) : null}
                  </span>
                  <span className="design-skill-mode-option-blurb">{copy.blurb}</span>
                </button>
              );
            })}
          </div>
          <button className="design-skill-picker-action" type="button" onClick={openCraft}>
            {skillSelection.mode === "manual" ? "Choose sections…" : "Read more"}
          </button>
        </div>
      ) : null}
    </div>
  );
});

// Exported for the modal-contract walking test in src/app/modals-over-crescent.test.tsx.
export const DesignCraftSheet = memo(function DesignCraftSheet({
  skillIndex,
  skillSelection,
  selectedSkillSlugs,
  resolvedSkillSlugs,
  appliedSkillSlugs,
  hasResolvedComposition,
  skillBlock,
  resolvedSkillSlugSet,
  automaticBaselineSlugSet,
  droppedSkillSlugSet,
  open,
  readOnly,
  onClose,
  onSkillToggle,
}: DesignCraftSheetProps) {
  const [expandedSlug, setExpandedSlug] = useState<string | null>(null);
  const modeCopy = SKILL_MODE_LABELS[skillSelection.mode];

  useModalOpen(open);

  // The sheet is always mounted; a fresh open must not inherit the last
  // one's expanded section. Adjusted during render, before paint.
  const [wasOpen, setWasOpen] = useState(false);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) setExpandedSlug(null);
  }

  // The sheet's Escape lives here with it, like the picker's and the
  // popover's: the parent owns the open state, the sheet owns its dismissal.
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, onClose]);
  const manualLimitReached =
    skillSelection.mode === "manual" && selectedSkillSlugs.length >= MAX_AUTOMATIC_SKILL_SECTIONS;
  const includedSkillCount = hasResolvedComposition
    ? Math.max(0, (resolvedSkillSlugs?.length ?? 0) - skillBlock.dropped.length)
    : 0;
  const droppedEntries = skillIndex.filter((entry) => {
    const isRequested = resolvedSkillSlugSet.has(entry.slug);
    return hasResolvedComposition && isRequested && droppedSkillSlugSet.has(entry.slug);
  });
  const expandedEntry = skillIndex.find((entry) => entry.slug === expandedSlug) ?? null;
  const isWaitingForAutomaticChoice = skillSelection.mode === "auto" && appliedSkillSlugs === null;
  const budgetHeading = hasResolvedComposition
    ? `${includedSkillCount} sections included`
    : "Automatic selection";
  const budgetValue = hasResolvedComposition
    ? `${skillBlock.totalChars.toLocaleString()} / ${skillBlock.ceiling.toLocaleString()} characters`
    : `up to ${MAX_AUTOMATIC_SKILL_SECTIONS} sections · ${skillBlock.ceiling.toLocaleString()}-character budget`;

  if (!open) return null;

  return (
    <div className="design-craft-overlay">
      <section
        className={`design-craft-sheet${expandedEntry !== null ? " design-craft-sheet-expanded" : ""}`}
        role="dialog"
        aria-labelledby="design-craft-sheet-title"
        aria-describedby="design-craft-sheet-budget"
      >
        <header className="design-craft-sheet-header">
          <div className="design-craft-sheet-heading">
            <h2 id="design-craft-sheet-title">Craft</h2>
            <span>{modeCopy.name}</span>
          </div>
          <div className="design-craft-budget" id="design-craft-sheet-budget" role="status">
            <strong>{budgetHeading}</strong>
            <span>{budgetValue}</span>
          </div>
          <button
            className="design-craft-close"
            type="button"
            aria-label="Close Craft"
            onClick={onClose}
          >
            ×
          </button>
        </header>

        <div className="design-craft-sheet-content">
          <div className="design-craft-index">
            <div className="design-craft-index-heading">
              <span>{readOnly ? "Sections" : "Choose sections"}</span>
              {!readOnly ? (
                <span className={manualLimitReached ? "design-craft-count-limit" : ""}>
                  {selectedSkillSlugs.length} / {MAX_AUTOMATIC_SKILL_SECTIONS}
                </span>
              ) : null}
            </div>
            {readOnly && droppedEntries.length > 0 ? (
              <p className="design-craft-budget-note">
                {droppedEntries.length} sections left out; the character budget is full.
              </p>
            ) : null}
            {readOnly && isWaitingForAutomaticChoice ? (
              <p className="design-craft-budget-note">
                The agent will choose sections for this request.
              </p>
            ) : null}
            {!readOnly && manualLimitReached ? (
              <p className="design-craft-budget-note design-craft-budget-note-limit">
                Maximum reached. Clear one to choose another.
              </p>
            ) : null}
            <ul className="design-craft-title-list">
              {skillIndex.map((entry) => {
                const isSelected = selectedSkillSlugs.includes(entry.slug);
                const isAutomaticBaseline =
                  skillSelection.mode === "auto" && automaticBaselineSlugSet.has(entry.slug);
                const isRequested = resolvedSkillSlugSet.has(entry.slug);
                const isDropped =
                  hasResolvedComposition && isRequested && droppedSkillSlugSet.has(entry.slug);
                const isIncluded =
                  !isDropped && ((hasResolvedComposition && isRequested) || isAutomaticBaseline);
                const isAutomaticallyUnselected =
                  skillSelection.mode === "auto" &&
                  appliedSkillSlugs !== null &&
                  !isRequested &&
                  !isAutomaticBaseline;
                const status = isDropped
                  ? "Left out"
                  : isAutomaticBaseline
                    ? "Always included"
                    : isWaitingForAutomaticChoice
                      ? "Chosen per request"
                      : isAutomaticallyUnselected
                        ? "Not chosen"
                        : isIncluded
                          ? "Included"
                          : "Not selected";
                const rowClass = [
                  "design-craft-title-row",
                  isSelected ? "design-craft-title-row-selected" : null,
                  isIncluded ? "design-craft-title-row-included" : null,
                  isDropped ? "design-craft-title-row-dropped" : null,
                  isAutomaticallyUnselected ? "design-craft-title-row-not-selected" : null,
                ]
                  .filter((className): className is string => className !== null)
                  .join(" ");
                const detailId = `design-craft-detail-${entry.slug}`;

                return (
                  <li className={rowClass} key={entry.slug}>
                    {readOnly ? (
                      <span
                        className={`design-craft-title-mark design-craft-title-mark-${
                          isDropped ? "dropped" : isIncluded ? "included" : "pending"
                        }`}
                        aria-hidden="true"
                      />
                    ) : (
                      <input
                        type="checkbox"
                        aria-label={`Apply ${entry.title}`}
                        checked={isSelected}
                        disabled={manualLimitReached && !isSelected}
                        onChange={() => {
                          if (!manualLimitReached || isSelected) onSkillToggle(entry.slug);
                        }}
                      />
                    )}
                    <button
                      className="design-craft-title-button"
                      type="button"
                      aria-expanded={expandedSlug === entry.slug}
                      aria-controls={detailId}
                      onClick={() =>
                        setExpandedSlug((current) => (current === entry.slug ? null : entry.slug))
                      }
                    >
                      <span>{entry.title}</span>
                      {readOnly ? (
                        <span className="design-craft-title-status">{status}</span>
                      ) : null}
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>

          {expandedEntry !== null ? (
            <article
              className="design-craft-detail"
              id={`design-craft-detail-${expandedEntry.slug}`}
            >
              <h3>{expandedEntry.title}</h3>
              <p>{expandedEntry.description}</p>
              <div className="design-craft-detail-body">{renderCraftBody(expandedEntry.body)}</div>
            </article>
          ) : null}
        </div>
      </section>
    </div>
  );
});
