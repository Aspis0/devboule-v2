export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    const clipboard = navigator.clipboard;
    if (clipboard === undefined) throw new Error("Clipboard unavailable");
    await clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
