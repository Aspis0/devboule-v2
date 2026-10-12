import { changeCountSides } from "./changeCountLabels";
import "./ChangeCounts.css";

/**
 * `+N −M` for the Changes list: compact, a zero side left out, in the diff
 * tones. An approximate count keeps the exact figures in its tooltip instead
 * of a mark beside the number.
 */
export function ChangesCounts({
  additions,
  deletions,
  approximate = false,
  className,
}: {
  additions: number;
  deletions: number;
  approximate?: boolean;
  className?: string;
}) {
  const { add, del } = changeCountSides(additions, deletions);
  const exact = `${additions.toLocaleString("en-US")} lines added, ${deletions.toLocaleString("en-US")} removed`;
  return (
    <span
      className={`workspace-changes-counts${className === undefined ? "" : ` ${className}`}`}
      title={approximate ? `Counts are approximate (${exact})` : undefined}
    >
      {add === null ? null : <span className="workspace-changes-counts-add">{add}</span>}
      {del === null ? null : <span className="workspace-changes-counts-del">{del}</span>}
    </span>
  );
}
