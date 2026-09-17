// The human's peer-restriction tick on a profile, as broker tool names.
//
// The overlay is a deny list, never a surface: ticking it says "children
// created from this profile may not message peers or create further agents".
// It keeps the roster, which is the child's read-only view of its siblings.
// The daemon validates every name against its own broker table, so these two
// spellings must stay exactly the broker's (`MCP_SEND_MESSAGE_TOOL` and
// `MCP_CREATE_AGENT_TOOL` in `provider_catalog.rs`).

/** Sends a message to one live agent session. */
export const PEER_MESSAGE_TOOL = "devboule_send_message";

/** Creates a new agent session from a profile. */
export const PEER_CREATE_TOOL = "devboule_create_agent";

/** Both peer tools, in the order the overlay stores them. */
export const PEER_TOOLS: readonly string[] = [PEER_MESSAGE_TOOL, PEER_CREATE_TOOL];

/** The overlay a ticked profile saves: both names. Unticked saves nothing. */
export function toolOverlayForPeerRestriction(restricted: boolean): string[] {
  return restricted ? [...PEER_TOOLS] : [];
}

/**
 * The row badge for a stored overlay, or null when it denies nothing. The
 * exact peer pair keeps its human sentence; any other non-empty overlay
 * names what it denies, so a single-tool denial never reads as unrestricted.
 */
export function overlayDenialsDescription(
  toolOverlay: readonly string[] | undefined,
): string | null {
  if (toolOverlay === undefined || toolOverlay.length === 0) return null;
  const isPeerPair =
    toolOverlay.length === PEER_TOOLS.length &&
    PEER_TOOLS.every((tool) => toolOverlay.includes(tool));
  if (isPeerPair) {
    return "Children from this profile cannot message peers or create further agents.";
  }
  return `Children from this profile cannot use: ${[...toolOverlay].join(", ")}.`;
}
