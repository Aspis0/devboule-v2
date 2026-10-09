import { isBrowserToolRow } from "../../../lib/browserToolName";
import { terminalToolVerb } from "../daemonToolVerb";

/** The decorative glyph a tool line leads with; the verb after it names the call. */
export function ToolIcon({ kind, title }: { kind: string | undefined; title: string }) {
  const name = iconName(kind?.trim().toLowerCase(), title);
  return (
    <svg
      aria-hidden="true"
      className="workspace-chat-tool-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="2"
      focusable="false"
    >
      {PATHS[name]}
    </svg>
  );
}

function iconName(kind: string | undefined, title: string): keyof typeof PATHS {
  if (terminalToolVerb(title) !== undefined || kind === "execute") return "terminal";
  if (isBrowserToolRow(kind, title)) return "globe";
  switch (kind) {
    case "read":
      return "file";
    case "edit":
    case "delete":
      return "pencil";
    case "search":
      return "search";
    case "fetch":
      return "link";
    case "plan":
      return "list";
    case "think":
      return "sparkle";
    case "question":
      return "help";
    default:
      return "tool";
  }
}

const PATHS = {
  terminal: (
    <>
      <path d="m4 17 6-5-6-5" />
      <path d="M12 19h8" />
    </>
  ),
  globe: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" />
    </>
  ),
  file: (
    <>
      <path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z" />
      <path d="M14 3v6h6" />
    </>
  ),
  pencil: <path d="M4 20h4L19 9l-4-4L4 16z" />,
  search: (
    <>
      <circle cx="11" cy="11" r="7" />
      <path d="m20 20-4-4" />
    </>
  ),
  link: (
    <>
      <path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1" />
      <path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1" />
    </>
  ),
  list: <path d="M8 6h12M8 12h12M8 18h12M4 6h.01M4 12h.01M4 18h.01" />,
  sparkle: <path d="M12 3v4M12 17v4M3 12h4M17 12h4" />,
  help: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M9.5 9a2.5 2.5 0 0 1 5 .5c0 1.7-2.5 2-2.5 3.5M12 17h.01" />
    </>
  ),
  tool: (
    <path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.5 2.5-2.5-.5-.5-2.5z" />
  ),
};
