import type { SessionKind } from "../../../types/ipc";

const STROKE = {
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.4,
  strokeLinecap: "round",
  strokeLinejoin: "round",
} as const;

/** One 14 px kind mark per session kind, drawn as our own simple strokes. */
export function StripKindMark({ kind }: { kind: SessionKind }) {
  return (
    <span className="strip-kind" aria-hidden="true">
      <svg width={14} height={14} viewBox="0 0 14 14" aria-hidden="true" focusable="false">
        {mark(kind)}
      </svg>
    </span>
  );
}

function mark(kind: SessionKind) {
  switch (kind) {
    case "claude":
      return (
        <g data-mark="burst" {...STROKE}>
          <path d="M7 1v12M1 7h12M2.8 2.8l8.4 8.4M11.2 2.8L2.8 11.2" />
        </g>
      );
    case "codex":
      return (
        <g data-mark="hex-dot" {...STROKE}>
          <path d="M7 1.5L11.6 4.2v5.6L7 12.5 2.4 9.8V4.2L7 1.5z" />
          <circle cx="7" cy="7" r="1.2" fill="currentColor" stroke="none" />
        </g>
      );
    case "pi":
      return (
        <text data-mark="pi" x="7" y="10.5" textAnchor="middle" fontSize="10" fill="currentColor">
          π
        </text>
      );
    case "terminal":
      return (
        <g data-mark="terminal" {...STROKE}>
          <rect x="1.5" y="2.5" width="11" height="9" rx="1.5" />
          <path d="M4 5.2l2 1.8-2 1.8M7.2 9h2.6" />
        </g>
      );
    default:
      return (
        <g data-mark="agent" {...STROKE}>
          <circle cx="7" cy="7" r="4.5" />
          <path d="M3.8 10.2L10.2 3.8" />
          <circle cx="7" cy="7" r="1.2" fill="currentColor" stroke="none" />
        </g>
      );
  }
}
