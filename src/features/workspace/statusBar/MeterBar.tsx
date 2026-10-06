/** A thin usage bar. Decoration only: the number it draws is always in the text beside it. */
export function MeterBar({ percent }: { percent: number | null }) {
  return (
    <span className="status-meter-bar" aria-hidden="true">
      {percent === null ? null : (
        <span
          className="status-meter-fill"
          style={{ width: `${Math.max(0, Math.min(100, percent))}%` }}
        />
      )}
    </span>
  );
}
