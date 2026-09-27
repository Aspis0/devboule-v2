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
  | "panel"
  | "kebab"
  | "chevron-right";

function paths(name: PanelIconName): ReactNode {
  switch (name) {
    case "files":
      return <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />;
    case "changes":
      return (
        <>
          <path d="M6 3v12" />
          <circle cx="18" cy="6" r="3" />
          <circle cx="6" cy="18" r="3" />
          <path d="M18 9a9 9 0 0 1-9 9" />
        </>
      );
    case "design":
      return (
        <>
          <path d="M4 20l1-4L16.5 4.5a2.1 2.1 0 0 1 3 3L8 19l-4 1z" />
          <path d="M14.5 6.5l3 3" />
        </>
      );
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
          <circle cx="12" cy="5" r="1.3" fill="currentColor" stroke="none" />
          <circle cx="12" cy="12" r="1.3" fill="currentColor" stroke="none" />
          <circle cx="12" cy="19" r="1.3" fill="currentColor" stroke="none" />
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
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {paths(name)}
    </svg>
  );
}
