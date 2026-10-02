import type { HostDotTone } from "./sidebarHosts";

export interface HostSectionHeadProps {
  name: string;
  dot: HostDotTone;
  /**
   * The status word beside the dot. `null` where the header has always been the
   * name and the dot alone — with one host there is nothing for a word to
   * tell apart.
   */
  word: string | null;
  collapsed?: boolean;
  /** Absent for a header with no body to hide, which is not a control. */
  onToggle?: () => void;
}

/**
 * One host section's header: the monitor glyph, the host's name, its status
 * word and dot, and — for a section that has a body — the whole row as the
 * collapse toggle.
 */
export function HostSectionHead({
  name,
  dot,
  word,
  collapsed = false,
  onToggle,
}: HostSectionHeadProps) {
  const row = (
    <>
      <svg
        className="sidebar-host-icon"
        viewBox="0 0 24 24"
        aria-hidden="true"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <rect width="20" height="14" x="2" y="3" rx="2" />
        <path d="M8 21h8" />
        <path d="M12 17v4" />
      </svg>
      {name}
      <span className="sidebar-top-spacer" />
      {word === null ? null : <span className="sidebar-host-status">{word}</span>}
      <span className={`workspace-status-dot workspace-dot-${dot}`} />
    </>
  );
  if (onToggle === undefined) {
    return <div className="sidebar-host-head">{row}</div>;
  }
  return (
    <button
      type="button"
      className="sidebar-host-head"
      aria-expanded={!collapsed}
      onClick={onToggle}
    >
      {row}
    </button>
  );
}
