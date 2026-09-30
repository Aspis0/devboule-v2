// The command row's chrome inside a tool-row summary: the kind word, the chip
// that carries the command's payload, and the marker that carries its exit code.

/** The payload shown in the command chip, taken from the row summary. */
export function CommandChip({ command }: { command: string }) {
  return (
    <>
      <span className="sr-only">Command</span>
      <span className="workspace-command-chip" title={command}>
        {command}
      </span>
    </>
  );
}

/** The exit dot and the sentence that reads its number: the dot is decoration
 * (`aria-hidden`), and the colour is never the only carrier of the code. */
export function ExitMarker({ exitCode }: { exitCode: number }) {
  return (
    <>
      <span
        className={`workspace-command-dot${exitCode === 0 ? "" : " is-failed"}`}
        aria-hidden="true"
      />
      <span className="workspace-command-exit">exit {exitCode}</span>
    </>
  );
}
