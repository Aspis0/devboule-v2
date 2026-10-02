import { useMemo, type ReactNode } from "react";
import type { DaemonStatus } from "../../../types/ipc";
import { usePairedDevices } from "../workspaceDaemon";
import { HostSectionHead } from "./HostSectionHead";
import { orderRemoteHosts, sidebarHosts } from "./sidebarHosts";
import { useHostRegistry } from "./useHostRegistry";

/**
 * Nothing of a remote host's workspaces is in the app yet, and saying so is
 * honest in a way an empty project list is not: an empty list under a live host
 * reads as a broken host.
 */
const REMOTE_BODY = "This host's workspaces are not available in this version.";

export interface HostSectionsProps {
  daemon: DaemonStatus;
  children: ReactNode;
}

/**
 * One host is not a list: with nothing to tell apart, the header keeps the name
 * and the dot it has always had. The grouping starts at two.
 */
export function HostSections({ daemon, children }: HostSectionsProps) {
  const devices = usePairedDevices();
  // Keyed on the state, not the daemon object: the status poll hands out a new
  // answer every 2 s and none of the rest of it reaches a host row.
  const seen = useMemo(() => sidebarHosts(daemon.state, devices), [daemon.state, devices]);
  const hostIds = useMemo(() => [seen.local.id, ...seen.remotes.map((host) => host.id)], [seen]);
  const { order, isCollapsed, toggle } = useHostRegistry(hostIds);
  const remotes = useMemo(() => orderRemoteHosts(seen.remotes, order), [seen, order]);

  if (remotes.length === 0) {
    return (
      <div className="sidebar-host">
        <HostSectionHead name={seen.local.name} dot={seen.local.status.dot} word={null} />
        {children}
      </div>
    );
  }

  return (
    <>
      {[seen.local, ...remotes].map((host) => {
        const collapsed = isCollapsed(host.id);
        const body = host.isLocal ? children : <p className="sidebar-host-note">{REMOTE_BODY}</p>;
        return (
          <section className="sidebar-host-section" key={host.id}>
            <HostSectionHead
              name={host.name}
              dot={host.status.dot}
              word={host.status.word}
              collapsed={collapsed}
              onToggle={() => toggle(host.id)}
            />
            {collapsed ? null : body}
          </section>
        );
      })}
    </>
  );
}
