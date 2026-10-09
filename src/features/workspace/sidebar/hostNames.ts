import type { PeerRow } from "../../../types/ipc";
import { LOCAL_HOST_ID, type HostId } from "../hosts/hostIdentity";
import type { PairedDevices } from "../workspaceDaemon";

/** The short label the rows print for the machine the app runs on. */
export const LOCAL_HOST_NAME = "This PC";

/**
 * The two answers a device list carries: what a person calls the device, and
 * the node name its own machine reported. The id is never one of them.
 */
const UNNAMED_DEVICE = "Unnamed device";

export function peerName(peer: PeerRow): string {
  const name = peer.displayName.trim();
  if (name !== "") return name;
  const node = peer.bindingNodeName?.trim() ?? "";
  return node !== "" ? node : UNNAMED_DEVICE;
}

/**
 * Every host's short label by id: the local machine first, then one per
 * paired peer that is a machine. A `client` peer views and steers this device
 * and is never dialled as a machine, so it is not a host; a revoked peer is
 * not paired at all.
 */
export function hostNames(devices: PairedDevices): ReadonlyMap<HostId, string> {
  const names = new Map<HostId, string>([[LOCAL_HOST_ID, LOCAL_HOST_NAME]]);
  for (const peer of devices.peers) {
    if (peer.role === "daemon" && peer.revokedAt === null) {
      names.set(peer.deviceId as HostId, peerName(peer));
    }
  }
  return names;
}
