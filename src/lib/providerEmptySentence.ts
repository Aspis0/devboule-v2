/** The Providers and Usage pages' shared no-CLI line, naming the PATH
    directories the daemon could not read instead of asserting their absence. */
export function providerEmptySentence(unreadableDirs: number): string {
  return unreadableDirs > 0
    ? `No agent CLI found, but ${unreadableDirs} PATH directories could not be read`
    : "No agent CLI found on PATH";
}
