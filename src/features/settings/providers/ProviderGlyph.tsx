import { StripKindMark } from "../../workspace/strip/StripKindMark";

/** Provider ids the strip draws its own mark for; anything else is generic. */
const KIND_BY_ID: Record<string, "claude" | "codex" | "pi"> = {
  claude: "claude",
  codex: "codex",
  pi: "pi",
};

/**
 * One provider glyph per row. The strip's kind marks for claude/codex/pi,
 * the strip's generic agent mark otherwise — never a brand logo. `"acp"`
 * names no case in the strip's switch, so it selects that generic branch
 * on purpose; the test pins `data-mark="agent"` for unknown ids.
 */
export function ProviderGlyph({ providerId }: { providerId: string }) {
  return <StripKindMark kind={KIND_BY_ID[providerId] ?? "acp"} />;
}
