import type { WorkspaceKey } from "../workspace/hosts/hostIdentity";

export interface TerminalSessionRecord {
  workspaceKey: WorkspaceKey | null;
  sessionId: string;
  lastSeenSeq: number | null;
}

export interface TerminalSessionRegistry {
  get: (workspaceKey: WorkspaceKey | null) => TerminalSessionRecord | null;
  register: (workspaceKey: WorkspaceKey | null, sessionId: string) => void;
  updateCursor: (workspaceKey: WorkspaceKey | null, sessionId: string, seq: number) => void;
  remove: (workspaceKey: WorkspaceKey | null, sessionId: string) => void;
}

function registryKey(workspaceKey: WorkspaceKey | null): string {
  return workspaceKey === null ? "workspace:null" : `workspace:${workspaceKey}`;
}

/**
 * Runtime-only ownership for terminal processes during one app run. This is a
 * module map instead of Zustand because React must never subscribe to terminal
 * output or session bookkeeping; components adopt/detach imperatively.
 */
export const terminalSessionRegistry: TerminalSessionRegistry = (() => {
  const sessions = new Map<string, TerminalSessionRecord>();

  return {
    get: (workspaceKey) => sessions.get(registryKey(workspaceKey)) ?? null,
    register: (workspaceKey, sessionId) => {
      sessions.set(registryKey(workspaceKey), {
        workspaceKey,
        sessionId,
        lastSeenSeq: null,
      });
    },
    updateCursor: (workspaceKey, sessionId, seq) => {
      const key = registryKey(workspaceKey);
      const record = sessions.get(key);
      if (record === undefined || record.sessionId !== sessionId) return;
      if (record.lastSeenSeq === null || seq > record.lastSeenSeq) {
        record.lastSeenSeq = seq;
      }
    },
    remove: (workspaceKey, sessionId) => {
      const key = registryKey(workspaceKey);
      const record = sessions.get(key);
      if (record?.sessionId === sessionId) sessions.delete(key);
    },
  };
})();
