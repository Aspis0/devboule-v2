export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    const clipboard = navigator.clipboard;
    // Non-secure contexts may lack clipboard access even though the DOM type says it exists.
    // Awaiting an absent method result would otherwise look like a successful copy.
    if (clipboard === undefined) throw new Error("Clipboard unavailable");
    await clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
