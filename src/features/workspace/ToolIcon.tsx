import type { ReactNode } from "react";
import type { ToolIconName } from "./toolRowDisplay";

interface ToolIconProps {
  name: ToolIconName;
  size?: number;
}

function paths(name: ToolIconName): ReactNode {
  switch (name) {
    case "terminal":
      return (
        <>
          <rect x="2" y="3" width="20" height="18" rx="2" />
          <path d="M6 9l3 3-3 3" />
          <path d="M12 15h5" />
        </>
      );
    case "eye":
      return (
        <>
          <path d="M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6-10-6-10-6z" />
          <circle cx="12" cy="12" r="2.5" />
        </>
      );
    case "pencil":
      return (
        <>
          <path d="M4 20l1-4L16.5 4.5a2.1 2.1 0 013 3L8 19l-4 1z" />
          <path d="M14.5 6.5l3 3" />
        </>
      );
    case "search":
      return (
        <>
          <circle cx="11" cy="11" r="6" />
          <path d="M15.5 15.5L20 20" />
        </>
      );
    case "bot":
      return (
        <>
          <rect x="5" y="9" width="14" height="11" rx="2" />
          <path d="M12 9V4" />
          <circle cx="12" cy="3" r="1" />
          <circle cx="9.5" cy="14" r="1" />
          <circle cx="14.5" cy="14" r="1" />
          <path d="M9.5 17h5" />
        </>
      );
    case "wrench":
      return (
        <path d="M14.5 6.5a4 4 0 015.2 3.8L16 14l-2-2 3.7-3.7a4 4 0 01-5.2-3.8l2.6 2.6 2-2L14.5 6.5zM9 13l-5 5 2 2 5-5" />
      );
  }
}

export function ToolIcon({ name, size = 14 }: ToolIconProps) {
  return (
    <svg
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
