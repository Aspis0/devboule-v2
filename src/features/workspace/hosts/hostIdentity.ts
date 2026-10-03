// Why branded strings and not a `{ hostId, workspaceId }` pair: every IPC call
// takes a bare daemon-minted id, and a pair would put a host on the wire.

/** A machine that owns workspaces: the local one today, a peer's device id later. */
export type HostId = string & { readonly __brand: "HostId" };

/** A workspace's identity WITHIN a host — never a value the daemon is sent. */
export type WorkspaceKey = string & { readonly __brand: "WorkspaceKey" };

/** The machine the app runs on: the one id the sidebar's registry and the tab stores share. */
export const LOCAL_HOST_ID = "local" as HostId;

const SEPARATOR = ":";

/** A blank half is refused: such a key would parse back as a different pair. */
export function workspaceKey(hostId: HostId, workspaceId: string): WorkspaceKey | null {
  if (hostId === "" || workspaceId === "") return null;
  return `${hostId}${SEPARATOR}${workspaceId}` as WorkspaceKey;
}

export function localWorkspaceKey(workspaceId: string): WorkspaceKey | null {
  return workspaceKey(LOCAL_HOST_ID, workspaceId);
}

/**
 * Splits at the FIRST separator: the host is the prefix this app minted, the
 * workspace id is the daemon's, so only the workspace half may carry one.
 * Total — every caller passes a key this module composed.
 */
export function parseWorkspaceKey(key: WorkspaceKey): { hostId: HostId; workspaceId: string } {
  const cut = key.indexOf(SEPARATOR);
  return { hostId: key.slice(0, cut) as HostId, workspaceId: key.slice(cut + 1) };
}

/** The rule `workspaceKey` applies, for text read back from storage. */
export function isWorkspaceKey(value: unknown): value is WorkspaceKey {
  if (typeof value !== "string") return false;
  const cut = value.indexOf(SEPARATOR);
  return cut > 0 && cut < value.length - 1;
}
