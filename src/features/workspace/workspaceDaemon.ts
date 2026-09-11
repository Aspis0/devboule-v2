import { useEffect, useState } from "react";
import { daemonStatus } from "../../lib/tauri";
import type { DaemonStatus } from "../../types/ipc";

// The supervisor itself reports "connecting" before its first answer, so the
// footer must not claim a verdict it has not heard yet.
const CONNECTING_DAEMON: DaemonStatus = {
  state: "connecting",
  pid: null,
  instanceId: null,
  protocolVersion: null,
  clients: null,
  capabilities: [],
  message: null,
};

const DISCONNECTED_DAEMON: DaemonStatus = {
  state: "disconnected",
  pid: null,
  instanceId: null,
  protocolVersion: null,
  clients: null,
  capabilities: [],
  message: "daemon unreachable",
};

export function useWorkspaceDaemon(): DaemonStatus {
  const [daemon, setDaemon] = useState<DaemonStatus>(CONNECTING_DAEMON);

  useEffect(() => {
    let cancelled = false;
    const tick = () => {
      void daemonStatus()
        .then((next) => {
          if (!cancelled) setDaemon(next);
        })
        .catch(() => {
          if (!cancelled) setDaemon(DISCONNECTED_DAEMON);
        });
    };
    tick();
    const id = window.setInterval(tick, 2000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  return daemon;
}
