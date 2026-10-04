/**
 * What a provider calls one of the browser lane's tools, or null for anything
 * else.
 *
 * A provider calls a tool by the name it was handed: the pi bridge and an ACP
 * agent pass the broker's own (`browser_click`), while Claude qualifies a tool
 * with the MCP server it came from (`mcp__devboule__browser_click`). Only this
 * daemon's own server qualifier comes off — another MCP server's `browser_click`
 * is that server's tool, which this daemon cannot serve and must not show as
 * its own. The qualifier has to stand at the front: `my_browser_click` is
 * another tool's name that happens to end in ours.
 *
 * Both literals are the daemon's (`provider_catalog::BROWSER_TOOL_PREFIX` and
 * `mcp_broker::MCP_SERVER_NAME`), and the browser row's tests read both back out
 * of the Rust source, so a rename there fails a test here.
 *
 * What comes back is the prefix match, not a check against the served table:
 * the table is the daemon's. A name this answers for but the broker does not
 * serve can only be a name an agent invented, and it shows as the lane with no
 * summary, which is better than the invented name.
 */
const BROWSER_PREFIX = "browser_";
const MCP_SERVER_QUALIFIER = "mcp__devboule__";
const SERVER_QUALIFIER = "devboule_";

export function browserToolName(name: string): string | null {
  const bare = name.trim().replace(MCP_SERVER_QUALIFIER, "").replace(SERVER_QUALIFIER, "");
  return bare.startsWith(BROWSER_PREFIX) && bare.length > BROWSER_PREFIX.length ? bare : null;
}

/**
 * Whether one chat row is a browser call, from its kind and its title.
 *
 * A kind the daemon named wins: `search` is a claim about what the row is, and
 * a grep whose query reads `browser_click` is a grep. `other` is nobody's claim
 * — the fallback for a tool nobody mapped — and so is no kind at all, so there
 * the title decides. That is what a row journaled before the daemon learned
 * Claude's spelling needs: it carries `other` and `mcp__devboule__browser_x`.
 *
 * The row and a run's count both ask this, so a browser call cannot show as one
 * row and summarize as another.
 */
export function isBrowserToolRow(kind: string | undefined, title: string): boolean {
  const named = kind?.trim().toLowerCase();
  if (named !== undefined && named !== "" && named !== "other") return named === "browser";
  return browserToolName(title) !== null;
}
