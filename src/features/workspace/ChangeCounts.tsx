import { countParts } from "./changeCountParts";
import "./ChangeCounts.css";

/** `+N −M` in the diff tones, a zero side left out (see `countParts`). */
export function ChangeCounts({ additions, deletions }: { additions: number; deletions: number }) {
  const { add, del } = countParts(additions, deletions);
  return (
    <span className="workspace-changes-counts">
      {add === null ? null : <span className="workspace-changes-counts-add">{add}</span>}
      {del === null ? null : <span className="workspace-changes-counts-del">{del}</span>}
    </span>
  );
}
