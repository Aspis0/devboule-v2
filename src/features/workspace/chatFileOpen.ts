import type { ChatFileTarget } from "../../lib/chatFilePaths";
import type { WorkspaceEditableFile } from "../../types/ipc";

// The editor's answer for a folder. It is the only folder signal the open gives,
// so the chat matches the daemon's sentence to tell a folder from a file.
const FOLDER_REFUSAL = "the requested path is a folder, not a file";

/**
 * Opens a path a chat link names. The editor is asked first: a folder answers
 * "folder" and no tab opens; a file, or any other answer, opens the file tab as before.
 */
export async function openChatFileLink(
  relativePath: string,
  probe: (path: string) => Promise<WorkspaceEditableFile>,
  openTab: (path: string) => void,
): Promise<ChatFileTarget> {
  let answer: WorkspaceEditableFile;
  try {
    answer = await probe(relativePath);
  } catch {
    openTab(relativePath);
    return "file";
  }
  if (answer.status === "refused" && answer.error === FOLDER_REFUSAL) return "folder";
  openTab(relativePath);
  return "file";
}
