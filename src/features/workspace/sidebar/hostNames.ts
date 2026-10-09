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
 * A row's host label: the known short name, never a raw device id. A revoked
 * or forgotten peer is not in the map, and an id is an identity, not a name.
 */
export function hostLabel(names: ReadonlyMap<HostId, string>, hostId: HostId): string {
  return names.get(hostId) ?? (hostId === LOCAL_HOST_ID ? LOCAL_HOST_NAME : "Unknown host");
}

/**
 * Every paired host's short label by id: the local machine first, then one per
 * paired, non-revoked peer. Pairing records no role and the slice-1 wire has
 * no field that says whether a peer hosts workspaces, so every paired device
 * is a candidate; a peer with no workspaces renders no rows and therefore no
 * section, because the tree is built from workspaces, not from this map.
 */
export function hostNames(devices: PairedDevices): ReadonlyMap<HostId, string> {
  const names = new Map<HostId, string>([[LOCAL_HOST_ID, LOCAL_HOST_NAME]]);
  for (const peer of devices.peers) {
    if (peer.revokedAt === null) {
      names.set(peer.deviceId as HostId, peerName(peer));
    }
  }
  return names;
}
