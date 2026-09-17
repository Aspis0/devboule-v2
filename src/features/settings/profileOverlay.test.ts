import { describe, expect, it } from "vitest";
import {
  PEER_CREATE_TOOL,
  PEER_MESSAGE_TOOL,
  overlayDenialsDescription,
  profileRestrictsPeers,
  toolOverlayForPeerRestriction,
} from "./profileOverlay";

describe("profile peer restriction", () => {
  it("names exactly the two peer tools the broker gates", () => {
    expect(PEER_MESSAGE_TOOL).toBe("devboule_send_message");
    expect(PEER_CREATE_TOOL).toBe("devboule_create_agent");
  });

  it("sends both names when ticked and nothing when unticked", () => {
    expect(toolOverlayForPeerRestriction(true)).toEqual([
      "devboule_send_message",
      "devboule_create_agent",
    ]);
    expect(toolOverlayForPeerRestriction(false)).toEqual([]);
  });

  it("reads the restriction back off a stored overlay", () => {
    expect(profileRestrictsPeers(["devboule_send_message", "devboule_create_agent"])).toBe(true);
    expect(profileRestrictsPeers([])).toBe(false);
    expect(profileRestrictsPeers(undefined)).toBe(false);
    expect(profileRestrictsPeers(["devboule_send_message"])).toBe(false);
  });

  it("renders what a stored overlay denies, not just the ticked shape", () => {
    // The exact peer pair keeps its human sentence; a single-tool overlay —
    // which the daemon accepts — must name its denial instead of vanishing.
    expect(overlayDenialsDescription(["devboule_send_message", "devboule_create_agent"])).toBe(
      "Children from this profile cannot message peers or create further agents.",
    );
    expect(overlayDenialsDescription(["devboule_create_agent"])).toBe(
      "Children from this profile cannot use: devboule_create_agent.",
    );
    expect(overlayDenialsDescription([])).toBeNull();
    expect(overlayDenialsDescription(undefined)).toBeNull();
  });
});
