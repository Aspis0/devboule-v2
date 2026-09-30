import { memo } from "react";
import { userTurnLabel, type UserTurn } from "./turnGrouping";

/** Constructed once at module scope: building an Intl formatter resolves
 * locale data, which no per-render call should pay. */
const CLOCK_TIME = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
});
const DATE_AND_TIME = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});

/** The local day an instant falls on, `YYYY-MM-DD` — the key "today"
 * is compared against. */
export function dayKey(ms: number): string {
  const date = new Date(ms);
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${month}-${day}`;
}

/** The turn's clock when its send time falls on `today` — the rail
 * render's day key, so every dot in one render classifies the day the
 * same way — the date (year included) beside the time otherwise. */
function turnTime(atMs: number | undefined, today: string): string | null {
  // The repo's rule for a nullable ms (relativeTime.ts, historyGrouping.ts):
  // null, NaN and infinity are "no time", never a 1970 date on the card.
  if (typeof atMs !== "number" || !Number.isFinite(atMs)) return null;
  const at = new Date(atMs);
  // Finite but out of the Date range formats as an Invalid Date and throws inside Intl.
  if (Number.isNaN(at.getTime())) return null;
  return dayKey(at.getTime()) === today ? CLOCK_TIME.format(at) : DATE_AND_TIME.format(at);
}

interface TurnStopProps {
  turn: UserTurn;
  index: number;
  count: number;
  center: number;
  fits: boolean;
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
  fits,
  isCurrent,
  isOpen,
  tabIndex,
  today,
  jumpTo,
  openFromFocus,
  pressStarted,
  closePreview,
}: TurnStopProps) {
  const time = turnTime(turn.atMs, today);
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
