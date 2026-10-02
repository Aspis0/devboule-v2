import { describe, expect, it } from "vitest";
import type { DaemonStatus, PeerRow } from "../../../types/ipc";
import type { PairedDevices } from "../workspaceDaemon";
import { sidebarHosts } from "./sidebarHosts";

const CONNECTED: DaemonStatus = {
  state: "connected",
  pid: 42,
  instanceId: "daemon-test",
  protocolVersion: 21,
  clients: 1,
  capabilities: [],
  message: null,
};

function daemon(state: DaemonStatus["state"]): DaemonStatus {
  return { ...CONNECTED, state };
}

function peer(overrides: Partial<PeerRow> = {}): PeerRow {
  return {
    deviceId: "peer-1",
    displayName: "Studio",
    role: "daemon",
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

describe("the sidebar's host list", () => {
  it("is this PC alone when no daemon peer is paired", () => {
    const hosts = sidebarHosts(CONNECTED, devices([]));

    expect(hosts.remotes).toEqual([]);
    expect(hosts.local.name).toBe("This PC");
    expect(hosts.local.isLocal).toBe(true);
    expect(hosts.local.status).toEqual({ word: "online", dot: "green" });
  });

  it("never lists a client peer, and never a revoked one", () => {
    const hosts = sidebarHosts(
      CONNECTED,
      devices([
        peer({ deviceId: "peer-client", displayName: "Phone", role: "client" }),
        peer({ deviceId: "peer-revoked", displayName: "Old box", revokedAt: 99 }),
      ]),
    );

    expect(hosts.remotes).toEqual([]);
  });

  it("counts a daemon peer as a host, online or not", () => {
    const online = sidebarHosts(CONNECTED, devices([peer({ displayName: "Alpha" })]));
    const offline = sidebarHosts(
      CONNECTED,
      devices([peer({ displayName: "Alpha", online: false })]),
    );

    expect(online.remotes).toHaveLength(1);
    expect(online.remotes[0]?.name).toBe("Alpha");
    expect(online.remotes[0]?.status).toEqual({ word: "online", dot: "green" });
    expect(offline.remotes[0]?.status).toEqual({ word: "offline", dot: "border" });
  });

  it("says it cannot tell when the poll that carried the rows failed", () => {
    const hosts = sidebarHosts(CONNECTED, devices([peer({ displayName: "Alpha" })], true));

    // The row is kept: one missed poll is not evidence the device is gone.
    expect(hosts.remotes).toHaveLength(1);
    expect(hosts.remotes[0]?.status).toEqual({ word: "unknown", dot: "border" });
  });

  it("orders remote hosts by name, whatever order the peers arrived in", () => {
    const hosts = sidebarHosts(
      CONNECTED,
      devices([
        peer({ deviceId: "peer-z", displayName: "Zeta" }),
        peer({ deviceId: "peer-a", displayName: "alpha" }),
        peer({ deviceId: "peer-m", displayName: "Mika" }),
      ]),
    );

    expect(hosts.remotes.map((host) => host.name)).toEqual(["alpha", "Mika", "Zeta"]);
  });

  it("falls back through the tailnet node name to the device id for a nameless peer", () => {
    const hosts = sidebarHosts(
      CONNECTED,
      devices([
        peer({ deviceId: "peer-blank", displayName: "  ", bindingNodeName: " box.tailnet " }),
        peer({ deviceId: "peer-bare", displayName: "", bindingNodeName: null }),
      ]),
    );

    expect(hosts.remotes.map((host) => host.name)).toEqual(["box.tailnet", "peer-bare"]);
  });

  it("gives the local host the daemon's own words, dot aside", () => {
    expect(sidebarHosts(daemon("connecting"), devices([])).local.status).toEqual({
      word: "connecting",
      dot: "border",
    });
    expect(sidebarHosts(daemon("unresponsive"), devices([])).local.status).toEqual({
      word: "not answering",
      dot: "border",
    });
    expect(sidebarHosts(daemon("disconnected"), devices([])).local.status).toEqual({
      word: "offline",
      dot: "border",
    });
    expect(sidebarHosts(daemon("error"), devices([])).local.status).toEqual({
      word: "offline",
      dot: "border",
    });
  });
});
