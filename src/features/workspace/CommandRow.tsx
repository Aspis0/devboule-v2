// The command row's chrome inside a tool-row summary: the payload text and the
// marker that carries its exit code.

/** The payload shown after the verb, taken from the row summary. */
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

/** The failure mark and the sentence that reads its number: the mark is decoration
 * (`aria-hidden`), and the colour is never the only carrier of the code. A zero
 * exit is not drawn at all, so the mark always reads as a failure. */
export function ExitMarker({ exitCode }: { exitCode: number }) {
  return (
    <>
      <span className="workspace-command-dot" aria-hidden="true" />
      <span className="workspace-command-exit">exit {exitCode}</span>
    </>
  );
}
