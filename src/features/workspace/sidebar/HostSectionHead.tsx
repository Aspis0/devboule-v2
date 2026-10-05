import type { HostDotTone } from "./sidebarHosts";

/**
 * A host section's header: the host's name, its own status word and the dot,
 * folded by the click. There is one of these per host, and only when the rail
 * has more than one host to tell apart — a lone host gets no header at all
 * (HostSections).
 */
export interface HostSectionHeadProps {
  name: string;
  dot: HostDotTone;
  word: string;
  collapsed: boolean;
  onToggle: () => void;
}

export function HostSectionHead({ name, dot, word, collapsed, onToggle }: HostSectionHeadProps) {
  return (
    <button
      type="button"
      className="sidebar-host-head"
      aria-expanded={!collapsed}
      onClick={onToggle}
    >
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
      <span className="sidebar-host-status">{word}</span>
      <span className={`workspace-status-dot workspace-dot-${dot}`} />
    </button>
  );
}
