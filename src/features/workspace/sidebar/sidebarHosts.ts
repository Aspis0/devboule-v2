import type { DaemonStatus, PeerRow } from "../../../types/ipc";
import type { PairedDevices } from "../workspaceDaemon";

/** The dot's two tones: a host that answered for itself, and one that did not. */
export type HostDotTone = "green" | "border";

/** What a host header says beside its dot. */
export interface HostStatus {
  word: string;
  dot: HostDotTone;
}

/** One host of the sidebar: who it is, and whether it is answering. */
export interface SidebarHost {
  id: string;
  name: string;
  isLocal: boolean;
  status: HostStatus;
}

/** The machine the app runs on. Its id is this module's, not the daemon's. */
const LOCAL_HOST_ID = "local";

const LOCAL_HOST_NAME = "This PC";

/** The sidebar's host list: the local host first, then every remote one. */
export interface SidebarHosts {
  local: SidebarHost;
  remotes: SidebarHost[];
}

/**
 * The daemon's own connection state in the sidebar's short vocabulary. The
 * words match the sidebar foot's, so one state never reads two ways on one
 * screen.
 */
function localWord(state: DaemonStatus["state"]): string {
  switch (state) {
    case "connected":
      return "online";
    case "connecting":
      return "connecting";
    case "unresponsive":
      return "not answering";
    // Unreachable and errored read the same here, as they do in the foot.
    case "disconnected":
    case "error":
      return "offline";
  }
}

function localStatus(state: DaemonStatus["state"]): HostStatus {
  return { word: localWord(state), dot: state === "connected" ? "green" : "border" };
}

/**
 * A peer's liveness, which is all `devices_list` reports about it. `stale`
 * means the poll failed: the rows are the last known ones and nothing on them
 * is current, so the row says it cannot tell rather than repeating an online
 * flag from a previous poll.
 *
 * A peer row carries no credential state and no protocol version, so a host
 * header cannot claim either.
 */
function remoteStatus(peer: PeerRow, stale: boolean): HostStatus {
  if (stale) return { word: "unknown", dot: "border" };
  return { word: peer.online ? "online" : "offline", dot: peer.online ? "green" : "border" };
}

/**
 * The name the daemon recorded for a peer: `displayName` is what a person
 * entered at pairing, a cleared one falls back to the tailnet node the peer
 * bound to, and a peer with neither still gets its device id — a host header
 * must never render as a blank row.
 */
function peerName(peer: PeerRow): string {
  const name = peer.displayName.trim();
  if (name !== "") return name;
  const node = peer.bindingNodeName?.trim() ?? "";
  return node !== "" ? node : peer.deviceId;
}

/**
 * The sidebar's hosts: this PC, plus one per paired peer that is a machine.
 *
 * A `client` peer views and steers this device and is never dialled as a
 * machine, so it is not a host; a revoked peer is not paired at all. Remote
 * hosts are ordered by name so the list does not jump as statuses change.
 */
export function sidebarHosts(daemon: DaemonStatus, devices: PairedDevices): SidebarHosts {
  const remotes = devices.peers
    .filter((peer) => peer.role === "daemon" && peer.revokedAt === null)
    .map((peer) => ({
      id: peer.deviceId,
      name: peerName(peer),
      isLocal: false,
      status: remoteStatus(peer, devices.stale),
    }))
    .sort((a, b) => a.name.localeCompare(b.name));
  return {
    local: {
      id: LOCAL_HOST_ID,
      name: LOCAL_HOST_NAME,
      isLocal: true,
      status: localStatus(daemon.state),
    },
    remotes,
  };
}
