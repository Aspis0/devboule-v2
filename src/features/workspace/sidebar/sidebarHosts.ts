import type { DaemonConnectionState, PeerRow } from "../../../types/ipc";
import type { PairedDevices } from "../workspaceDaemon";

export type HostDotTone = "green" | "border";

export interface HostStatus {
  word: string;
  dot: HostDotTone;
}

export interface SidebarHost {
  id: string;
  name: string;
  isLocal: boolean;
  status: HostStatus;
}

/** The machine the app runs on. Its id is this module's, not the daemon's. */
const LOCAL_HOST_ID = "local";

const LOCAL_HOST_NAME = "This PC";

export interface SidebarHosts {
  local: SidebarHost;
  remotes: SidebarHost[];
}

/**
 * The two answers a device list carries: what a person calls the device, and
 * the node name its own machine reported. The id is never one of them.
 */
const UNNAMED_DEVICE = "Unnamed device";

function peerName(peer: PeerRow): string {
  const name = peer.displayName.trim();
  if (name !== "") return name;
  const node = peer.bindingNodeName?.trim() ?? "";
  return node !== "" ? node : UNNAMED_DEVICE;
}

/**
 * The words match the sidebar foot's, so one daemon state never reads two ways
 * on one screen.
 */
function localWord(state: DaemonConnectionState): string {
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

function localStatus(state: DaemonConnectionState): HostStatus {
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
 * A remembered order beats a name: a host the order has met keeps its place, a
 * host it has never met joins the end however its name sorts. Among hosts the
 * order has not met, the first-seen tie-break is the name and then the id, so
 * two peers that arrive in one poll cannot swap places under the next one.
 */
function byRememberedOrder(order: readonly string[]): (a: SidebarHost, b: SidebarHost) => number {
  const rank = new Map(order.map((hostId, index) => [hostId, index]));
  return (a, b) => {
    const ra = rank.get(a.id);
    const rb = rank.get(b.id);
    if (ra !== undefined && rb !== undefined) return ra - rb;
    if (ra !== undefined) return -1;
    if (rb !== undefined) return 1;
    return a.name.localeCompare(b.name) || a.id.localeCompare(b.id);
  };
}

/** The remote hosts in the order the registry remembers. */
export function orderRemoteHosts(
  remotes: readonly SidebarHost[],
  order: readonly string[],
): SidebarHost[] {
  return [...remotes].sort(byRememberedOrder(order));
}

/**
 * The sidebar's hosts: this PC, plus one per paired peer that is a machine.
 *
 * A `client` peer views and steers this device and is never dialled as a
 * machine, so it is not a host; a revoked peer is not paired at all. Remote
 * hosts arrive in name order and are re-ordered by `orderRemoteHosts` once the
 * registry knows them.
 */
export function sidebarHosts(state: DaemonConnectionState, devices: PairedDevices): SidebarHosts {
  const remotes = devices.peers
    .filter((peer) => peer.role === "daemon" && peer.revokedAt === null)
    .map((peer) => ({
      id: peer.deviceId,
      name: peerName(peer),
      isLocal: false,
      status: remoteStatus(peer, devices.stale),
    }))
    .sort(byRememberedOrder([]));
  return {
    local: {
      id: LOCAL_HOST_ID,
      name: LOCAL_HOST_NAME,
      isLocal: true,
      status: localStatus(state),
    },
    remotes,
  };
}
