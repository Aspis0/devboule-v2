/** A thin usage bar. Decoration only: the number it draws is always in the text beside it. */
export function MeterBar({ percent }: { percent: number | null }) {
  return (
    <span className="status-meter-bar" aria-hidden="true">
      {percent === null ? null : (
        <span
          className={`status-meter-fill${toneClass(percent)}`}
          style={{ width: `${Math.max(0, Math.min(100, percent))}%` }}
        />
      )}
    </span>
  );
}

/** Amber from 70 %, red past 90 %; these thresholds follow Paseo's usage tone. */
function toneClass(percent: number): string {
  if (percent > 90) return " status-meter-fill-danger";
  if (percent >= 70) return " status-meter-fill-warn";
  return "";
}
