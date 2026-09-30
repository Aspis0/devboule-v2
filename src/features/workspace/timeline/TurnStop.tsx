import { memo } from "react";
import { userTurnLabel, type UserTurn } from "./turnGrouping";

interface TurnStopProps {
  turn: UserTurn;
  index: number;
  count: number;
  center: number;
  fits: boolean;
  isCurrent: boolean;
  isOpen: boolean;
  jumpTo: (turn: UserTurn) => void;
  openFromFocus: (turn: UserTurn) => void;
  pressStarted: () => void;
  closePreview: () => void;
}

/**
 * One dot on the rail: its position, its label, its preview card.
 * Memoised — a preview opening on one dot, or the current turn moving,
 * may not re-render the other stops.
 */
function TurnStopInner({
  turn,
  index,
  count,
  center,
  fits,
  isCurrent,
  isOpen,
  jumpTo,
  openFromFocus,
  pressStarted,
  closePreview,
}: TurnStopProps) {
  return (
    <span
      className="turn-rail-stop"
      style={{ top: `${center}px` }}
      data-preview-fits={fits ? "" : undefined}
      data-preview-open={isOpen ? "" : undefined}
    >
      <button
        type="button"
        className="turn-rail-dot"
        aria-current={isCurrent ? "true" : undefined}
        aria-label={userTurnLabel(turn, index, count)}
        onClick={() => jumpTo(turn)}
        onPointerDown={pressStarted}
        onFocus={() => openFromFocus(turn)}
        onBlur={closePreview}
        onKeyDown={(event) => {
          // Focus stays on the dot: the rail is a list of stops and
          // the reader keeps moving through it. Suppressing the key's
          // default click keeps the jump one event, not two.
          if (event.key !== "Enter" && event.key !== " ") return;
          event.preventDefault();
          jumpTo(turn);
        }}
      >
        <span className="turn-rail-glyph" aria-hidden="true" />
        <span className="turn-rail-preview" aria-hidden="true">
          <span className="turn-rail-preview-title">{turn.title}</span>
        </span>
      </button>
    </span>
  );
}

export const TurnStop = memo(TurnStopInner);
