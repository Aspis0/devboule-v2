import type { ReactNode } from "react";

// The right panel's own glyphs: 14 px stroke icons beside the tab labels
// and in the kebab menu. One set so the tabs and the menu agree; plugin
// panels that bring no icon use "panel".
export type PanelIconName =
  | "files"
  | "changes"
  | "design"
  | "app"
  | "pr"
  | "tasks"
  | "panel"
  | "kebab"
  | "chevron-right";

function paths(name: PanelIconName): ReactNode {
  switch (name) {
    // The tab glyphs are the mockup's own (a-workspace-2.html:180-182): the
    // branch-row merge belongs to the Changes branch row (R7b), not the tab.
    case "files":
      return (
        <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />
      );
    case "changes":
      return (
        <>
          <rect x="3" y="3" width="18" height="18" rx="2" />
          <path d="M12 3v18" />
        </>
      );
    case "design":
      return <path d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" />;
    case "app":
      return (
        <>
          <rect x="2" y="3" width="20" height="14" rx="2" />
          <path d="M8 21h8" />
          <path d="M12 17v4" />
        </>
      );
    case "pr":
      return (
        <>
          <circle cx="18" cy="6" r="3" />
          <circle cx="6" cy="18" r="3" />
          <path d="M13 6h3a2 2 0 0 1 2 2v7" />
          <path d="M6 9v12" />
        </>
      );
    case "tasks":
      return (
        <>
          <path d="M9 6h12" />
          <path d="M9 12h12" />
          <path d="M9 18h12" />
          <path d="M4 6h.01" />
          <path d="M4 12h.01" />
          <path d="M4 18h.01" />
        </>
      );
    case "panel":
      return (
        <>
          <rect x="3" y="3" width="18" height="18" rx="2" />
          <path d="M9 3v18" />
        </>
      );
    case "kebab":
      return (
        <>
          <circle cx="12" cy="5" r="1" />
          <circle cx="12" cy="12" r="1" />
          <circle cx="12" cy="19" r="1" />
        </>
      );
    case "chevron-right":
      return <path d="m9 18 6-6-6-6" />;
  }
}

export function PanelIcon({ name, size = 14 }: { name: PanelIconName; size?: number }) {
  return (
    <svg
      className="workspace-panel-icon"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {paths(name)}
    </svg>
  );
}
