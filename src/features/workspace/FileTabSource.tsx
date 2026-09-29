// The File tab's Source body: the loaded window as plain, selectable
// text beside a gutter numbered from the window's own first line — so an
// appended window keeps counting, and the numbers never claim more than
// what is on screen.

import type { WorkspaceFileContent } from "../../types/ipc";

export function FileTabSource({ window }: { window: WorkspaceFileContent }) {
  const content = window.content ?? "";
  const lines = content === "" ? [] : content.split(/\r\n|\r|\n/);
  // A window's content ends where its last line ends; the split's
  // trailing "" after a final newline is no line of the file.
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  const first = window.fromLine ?? 1;
  return (
    <pre className="workspace-file-tab-source">
      {lines.map((line, index) => (
        <span key={index} className="workspace-file-tab-source-row">
          <span className="workspace-file-tab-source-number" aria-hidden="true">
            {first + index}
          </span>
          <span className="workspace-file-tab-source-line">{line}</span>
        </span>
      ))}
    </pre>
  );
}
