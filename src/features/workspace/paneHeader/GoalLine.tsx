import { memo, useEffect, useId, useRef, useState } from "react";
import "./GoalLine.css";

/**
 * The pinned goal row under the pane header: the target mark, the "Goal"
 * label, the objective on one ellipsised line, and the chevron that reveals
 * a long goal inline. The row sits outside the transcript scrollport, so it
 * never scrolls; without a goal it renders nothing at all. The surface
 * remounts it per goal text, so a replaced goal arrives collapsed.
 */
export const GoalLine = memo(function GoalLine({ goal }: { goal: string | null }) {
  const [expanded, setExpanded] = useState(false);
  const [overflows, setOverflows] = useState(false);
  const textRef = useRef<HTMLSpanElement>(null);
  const textId = useId();

  useEffect(() => {
    const text = textRef.current;
    if (text === null) return;
    // The observer's initial fire measures the first paint; later fires
    // track resizes. happy-dom never fires, so tests drive it by hand.
    const measure = () => setOverflows(text.scrollWidth > text.clientWidth);
    const observer = new ResizeObserver(measure);
    observer.observe(text);
    return () => observer.disconnect();
  }, [goal]);

  if (goal === null) return null;
  // A wrapped goal no longer overflows, but the chevron is then the only
  // way back to one line: it stays while expanded.
  const showChevron = overflows || expanded;
  return (
    <div className={`goal-line${expanded ? " is-expanded" : ""}`} data-testid="goal-line">
      <svg
        className="goal-line-icon"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <circle cx="12" cy="12" r="10" />
        <circle cx="12" cy="12" r="6" />
        <circle cx="12" cy="12" r="2" />
      </svg>
      <span className="goal-line-label">Goal</span>
      <span ref={textRef} id={textId} className="goal-line-text" title={goal}>
        {goal}
      </span>
      {showChevron ? (
        <button
          type="button"
          className="goal-line-chevron"
          aria-expanded={expanded}
          aria-controls={textId}
          aria-label={expanded ? "Collapse the goal" : "Show the whole goal"}
          onClick={() => setExpanded((wasExpanded) => !wasExpanded)}
          data-testid="goal-line-toggle"
        >
          <svg
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
          >
            <path d="m6 9 6 6 6-6" />
          </svg>
        </button>
      ) : null}
    </div>
  );
});
