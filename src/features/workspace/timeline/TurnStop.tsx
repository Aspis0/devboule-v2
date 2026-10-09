import { memo } from "react";
import { formatDayClock } from "../../../lib/dayClock";
import { userTurnLabel, type UserTurn } from "./turnGrouping";

interface TurnStopProps {
  turn: UserTurn;
  index: number;
  count: number;
  center: number;
  isCurrent: boolean;
  isOpen: boolean;
  tabIndex: 0 | -1;
  /** The rail render's local day key: one decision every dot shares. */
  today: string;
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
  isCurrent,
  isOpen,
  tabIndex,
  today,
  jumpTo,
  openFromFocus,
  pressStarted,
  closePreview,
}: TurnStopProps) {
  const time = formatDayClock(turn.atMs, today);
  return (
    <span
      className="turn-rail-stop"
      style={{ top: `${center}px` }}
      data-preview-open={isOpen ? "" : undefined}
    >
      <button
        type="button"
        className="turn-rail-dot"
        aria-current={isCurrent ? "true" : undefined}
        aria-label={userTurnLabel(turn, index, count, time)}
        tabIndex={tabIndex}
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
          <span className="turn-rail-preview-title" title={turn.title}>
            {turn.title}
          </span>
          {time === null ? null : <span className="turn-rail-preview-time">{time}</span>}
        </span>
      </button>
    </span>
  );
}

export const TurnStop = memo(TurnStopInner);
