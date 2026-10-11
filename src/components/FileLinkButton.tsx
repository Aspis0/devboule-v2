import { useState } from "react";
import type { ChatFileLinks } from "../lib/chatFilePaths";

/**
 * A path in agent text that opens in the workspace. When the editor says the path
 * is a folder, nothing opens and the tooltip says so.
 */
export function FileLinkButton({
  label,
  relative,
  links,
}: {
  label: string;
  relative: string;
  links: ChatFileLinks;
}) {
  const [folder, setFolder] = useState(false);
  return (
    <button
      type="button"
      className="plan-markdown-file-link"
      title={folder ? "Folder" : relative}
      onClick={async () => {
        if ((await links.open(relative)) === "folder") setFolder(true);
      }}
    >
      {label}
    </button>
  );
}
