export interface SidebarFooterProps {
  onOpenSettings: () => void;
}

/**
 * The sidebar's bottom row: the Settings button. The daemon's status is in the
 * status bar. Creating a project lives at the top, with the other creation
 * commands.
 */
export function SidebarFooter({ onOpenSettings }: SidebarFooterProps) {
  return (
    <div className="workspace-sidebar-footer">
      {/* A group, not a toolbar: the buttons keep their own tab stops, so no
          toolbar keyboard pattern is claimed. */}
      <div className="sidebar-icon-row" role="group" aria-label="Sidebar">
        <button
          type="button"
          className="workspace-icon-button sidebar-icon-button"
          onClick={onOpenSettings}
          title="Settings"
          aria-label="Settings"
        >
          <svg
            className="sidebar-icon-glyph"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.75"
            strokeLinecap="round"
            aria-hidden="true"
            focusable="false"
          >
            <path d="M4 7h9M17 7h3M4 17h3M11 17h9" />
            <circle cx="15" cy="7" r="2" />
            <circle cx="9" cy="17" r="2" />
          </svg>
        </button>
      </div>
    </div>
  );
}
