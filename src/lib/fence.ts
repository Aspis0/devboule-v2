const COPYABLE_FENCE_TAGS = new Set([
  "sh",
  "bash",
  "shell",
  "zsh",
  "console",
  "powershell",
  "ps1",
  "cmd",
  "env",
  "dotenv",
]);

// A blank line before the closing fence leaves a trailing newline in the body;
// pasting that into a terminal would run the last line.
const TRAILING_BLANK_LINES = /(\r?\n[ \t]*)+$/;

export function stripTrailingBlankLines(body: string): string {
  return body.replace(TRAILING_BLANK_LINES, "");
}

export function isCopyableFence(info: string | undefined, body: string): boolean {
  const tag = info?.trim().toLowerCase() ?? "";
  if (tag !== "") return COPYABLE_FENCE_TAGS.has(tag.split(/\s+/)[0]);
  return stripTrailingBlankLines(body).split("\n").length <= 3;
}
