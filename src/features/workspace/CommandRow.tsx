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

/** A non-zero exit, as a muted number. It is not a verdict: whether the call
 * failed is the provider's status (see `isFailedToolCall`), never this code. */
export function ExitMarker({ exitCode }: { exitCode: number }) {
  return <span className="workspace-command-exit">exit {exitCode}</span>;
}
