import { describe, expect, it } from "vitest";
import type { PeerRow } from "../../../types/ipc";
import { LOCAL_HOST_ID } from "../hosts/hostIdentity";
import type { PairedDevices } from "../workspaceDaemon";
import { LOCAL_HOST_NAME, hostLabel, hostNames, peerName } from "./hostNames";

function peer(overrides: Partial<PeerRow> = {}): PeerRow {
  return {
    deviceId: "peer-1",
    displayName: "Studio",
    publicKey: "k",
    keyFingerprint: "aaaa bbbb",
    bindingKind: "tailnet",
    bindingNodeName: "studio.tailnet",
    bindingLoginName: null,
    address: "100.64.0.9:47831",
    pairedAt: 1,
    revokedAt: null,
    caps: ["view"],
    pairedByUser: null,
    online: true,
    ...overrides,
  };
}

function devices(peers: readonly PeerRow[], stale = false): PairedDevices {
  return { peers, stale };
}

describe("the hosts' short labels", () => {
  it("names the local machine first, alone when no peer is paired", () => {
    const names = hostNames(devices([]));

    expect([...names]).toEqual([[LOCAL_HOST_ID, "This PC"]]);
    expect(LOCAL_HOST_NAME).toBe("This PC");
  });

  it("lists every paired peer and never a revoked one", () => {
    const names = hostNames(
      devices([
        peer({ deviceId: "peer-phone", displayName: "Phone" }),
        peer({ deviceId: "peer-revoked", displayName: "Old box", revokedAt: 99 }),
      ]),
    );

    expect([...names]).toEqual([
      [LOCAL_HOST_ID, LOCAL_HOST_NAME],
      ["peer-phone", "Phone"],
    ]);
  });

  it("lists a paired peer by its chosen name", () => {
    const names = hostNames(devices([peer({ deviceId: "device-a", displayName: "Marcolenovo" })]));

    expect(names.get("device-a" as never)).toBe("Marcolenovo");
  });

  it("falls back to the tailnet node name, and to a neutral word before any id", () => {
    expect(peerName(peer({ displayName: "  ", bindingNodeName: " box.tailnet " }))).toBe(
      "box.tailnet",
    );
    expect(peerName(peer({ displayName: "", bindingNodeName: null }))).toBe("Unnamed device");
    // An id is an identity, not a name: it never reaches a label.
    expect(
      peerName(peer({ deviceId: "device-x", displayName: "", bindingNodeName: null })),
    ).not.toContain("device-");
  });

  it("labels an unknown host without its raw id", () => {
    const names = hostNames(devices([]));

    expect(hostLabel(names, LOCAL_HOST_ID)).toBe("This PC");
    expect(hostLabel(names, "peer-gone" as never)).toBe("Unknown host");
  });
});
