import { describe, expect, it } from "vitest";
import type { PeerRow } from "../../../types/ipc";
import type { PairedDevices } from "../workspaceDaemon";
import { orderRemoteHosts, sidebarHosts } from "./sidebarHosts";

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
    const hosts = sidebarHosts("connected", devices([]));

    expect(hosts.remotes).toEqual([]);
    expect(hosts.local.name).toBe("This PC");
    expect(hosts.local.isLocal).toBe(true);
    expect(hosts.local.status).toEqual({ word: "online", dot: "green" });
  });

  it("never lists a client peer, and never a revoked one", () => {
    const hosts = sidebarHosts(
      "connected",
      devices([
        peer({ deviceId: "peer-client", displayName: "Phone", role: "client" }),
        peer({ deviceId: "peer-revoked", displayName: "Old box", revokedAt: 99 }),
      ]),
    );

    expect(hosts.remotes).toEqual([]);
  });

  it("counts a daemon peer as a host, online or not", () => {
    const online = sidebarHosts("connected", devices([peer({ displayName: "Alpha" })]));
    const offline = sidebarHosts(
      "connected",
      devices([peer({ displayName: "Alpha", online: false })]),
    );

    expect(online.remotes).toHaveLength(1);
    expect(online.remotes[0]?.name).toBe("Alpha");
    expect(online.remotes[0]?.status).toEqual({ word: "online", dot: "green" });
    expect(offline.remotes[0]?.status).toEqual({ word: "offline", dot: "border" });
  });

  it("says it cannot tell when the poll that carried the rows failed", () => {
    const hosts = sidebarHosts("connected", devices([peer({ displayName: "Alpha" })], true));

    // The row is kept: one missed poll is not evidence the device is gone.
    expect(hosts.remotes).toHaveLength(1);
    expect(hosts.remotes[0]?.status).toEqual({ word: "unknown", dot: "border" });
  });

  it("orders remote hosts by name, whatever order the peers arrived in", () => {
    const hosts = sidebarHosts(
      "connected",
      devices([
        peer({ deviceId: "peer-z", displayName: "Zeta" }),
        peer({ deviceId: "peer-a", displayName: "alpha" }),
        peer({ deviceId: "peer-m", displayName: "Mika" }),
      ]),
    );

    expect(hosts.remotes.map((host) => host.name)).toEqual(["alpha", "Mika", "Zeta"]);
  });

  it("orders two hosts of the same name by their device id", () => {
    const first = sidebarHosts(
      "connected",
      devices([
        peer({ deviceId: "device-z", displayName: "Studio" }),
        peer({ deviceId: "device-a", displayName: "Studio" }),
      ]),
    );

    expect(first.remotes.map((host) => host.id)).toEqual(["device-a", "device-z"]);
    // The same peers in the other arrival order must land in the same order.
    const second = sidebarHosts(
      "connected",
      devices([
        peer({ deviceId: "device-a", displayName: "Studio" }),
        peer({ deviceId: "device-z", displayName: "Studio" }),
      ]),
    );
    expect(second.remotes.map((host) => host.id)).toEqual(["device-a", "device-z"]);
  });

  it("falls back to the tailnet node name, and to a neutral word before any id", () => {
    const hosts = sidebarHosts(
      "connected",
      devices([
        peer({ deviceId: "device-blank", displayName: "  ", bindingNodeName: " box.tailnet " }),
        peer({ deviceId: "device-bare", displayName: "", bindingNodeName: null }),
      ]),
    );

    expect(hosts.remotes.map((host) => host.name)).toEqual(["box.tailnet", "Unnamed device"]);
    // An id is an identity, not a name: it never reaches a header.
    expect(hosts.remotes.map((host) => host.name).join(" ")).not.toContain("device-");
  });

  it("gives the local host the daemon's own words, dot aside", () => {
    expect(sidebarHosts("connecting", devices([])).local.status).toEqual({
      word: "connecting",
      dot: "border",
    });
    expect(sidebarHosts("unresponsive", devices([])).local.status).toEqual({
      word: "not answering",
      dot: "border",
    });
    expect(sidebarHosts("disconnected", devices([])).local.status).toEqual({
      word: "offline",
      dot: "border",
    });
    expect(sidebarHosts("error", devices([])).local.status).toEqual({
      word: "offline",
      dot: "border",
    });
  });
});

describe("the order the sidebar remembers", () => {
  // Name order, as the device list hands them over: alpha, Mika, Zeta.
  const remotes = sidebarHosts(
    "connected",
    devices([
      peer({ deviceId: "device-z", displayName: "Zeta" }),
      peer({ deviceId: "device-m", displayName: "Mika" }),
      peer({ deviceId: "device-a", displayName: "alpha" }),
    ]),
  ).remotes;

  it("is the device list's own order when nothing is remembered", () => {
    expect(orderRemoteHosts(remotes, []).map((host) => host.id)).toEqual([
      "device-a",
      "device-m",
      "device-z",
    ]);
  });

  it("puts the hosts it knows first and appends a new one last", () => {
    // "Zeta" is the host we remember; "alpha" is new and sorts first by name.
    const ordered = orderRemoteHosts(remotes, ["device-z"]);

    expect(ordered.map((host) => host.id)).toEqual(["device-z", "device-a", "device-m"]);
  });

  it("brings a host back to its place after a poll without it", () => {
    const whileAway = remotes.filter((host) => host.id !== "device-m");

    expect(orderRemoteHosts(whileAway, ["device-z", "device-m", "device-a"])).toHaveLength(2);
    expect(
      orderRemoteHosts(whileAway, ["device-z", "device-m", "device-a"]).map((host) => host.id),
    ).toEqual(["device-z", "device-a"]);
    expect(
      orderRemoteHosts(remotes, ["device-z", "device-m", "device-a"]).map((host) => host.id),
    ).toEqual(["device-z", "device-m", "device-a"]);
  });

  it("orders two hosts first seen in one poll by name, then by device id", () => {
    const sameName = sidebarHosts(
      "connected",
      devices([
        peer({ deviceId: "device-z", displayName: "Studio" }),
        peer({ deviceId: "device-a", displayName: "Studio" }),
      ]),
    ).remotes;

    expect(orderRemoteHosts(sameName, ["device-b"]).map((host) => host.id)).toEqual([
      "device-a",
      "device-z",
    ]);
  });
});
