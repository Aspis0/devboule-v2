import { useMemo, useState, type ReactNode } from "react";
import type { DaemonStatus } from "../../../types/ipc";
import { usePairedDevices } from "../workspaceDaemon";
import { HostSectionHead } from "./HostSectionHead";
import { sidebarHosts } from "./sidebarHosts";

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

/** Which sections are folded, local UI state until the host list is persisted. */
function toggleCollapsed(current: ReadonlySet<string>, hostId: string): ReadonlySet<string> {
  const next = new Set(current);
  if (next.has(hostId)) next.delete(hostId);
  else next.add(hostId);
  return next;
}

/**
 * One host is not a list: with nothing to tell apart, the header keeps the name
 * and the dot it has always had. The grouping starts at two.
 */
export function HostSections({ daemon, children }: HostSectionsProps) {
  const devices = usePairedDevices();
  // Keyed on the state, not the daemon object: the status poll hands out a new
  // answer every 2 s and none of the rest of it reaches a host row.
  const hosts = useMemo(() => sidebarHosts(daemon.state, devices), [daemon.state, devices]);
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());

  if (hosts.remotes.length === 0) {
    return (
      <div className="sidebar-host">
        <HostSectionHead name={hosts.local.name} dot={hosts.local.status.dot} word={null} />
        {children}
      </div>
    );
  }

  return (
    <>
      {[hosts.local, ...hosts.remotes].map((host) => {
        const isCollapsed = collapsed.has(host.id);
        const body = host.isLocal ? children : <p className="sidebar-host-note">{REMOTE_BODY}</p>;
        return (
          <section className="sidebar-host-section" key={host.id}>
            <HostSectionHead
              name={host.name}
              dot={host.status.dot}
              word={host.status.word}
              collapsed={isCollapsed}
              onToggle={() => setCollapsed((open) => toggleCollapsed(open, host.id))}
            />
            {isCollapsed ? null : body}
          </section>
        );
      })}
    </>
  );
}
