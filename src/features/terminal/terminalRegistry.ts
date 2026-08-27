export interface TerminalSessionRecord {
  workspaceId: string | null;
  sessionId: string;
  lastSeenSeq: number | null;
}

export interface TerminalSessionRegistry {
  get: (workspaceId: string | null) => TerminalSessionRecord | null;
  register: (workspaceId: string | null, sessionId: string) => void;
  updateCursor: (workspaceId: string | null, sessionId: string, seq: number) => void;
  remove: (workspaceId: string | null, sessionId: string) => void;
}

function registryKey(workspaceId: string | null): string {
  return workspaceId === null ? 'workspace:null' : `workspace:${workspaceId}`;
}

/**
 * Runtime-only ownership for terminal processes during one app run. This is a
 * module map instead of Zustand because React must never subscribe to terminal
 * output or session bookkeeping; components adopt/detach imperatively.
 */
export const terminalSessionRegistry: TerminalSessionRegistry = (() => {
  const sessions = new Map<string, TerminalSessionRecord>();

  return {
    get: (workspaceId) => sessions.get(registryKey(workspaceId)) ?? null,
    register: (workspaceId, sessionId) => {
      sessions.set(registryKey(workspaceId), {
        workspaceId,
        sessionId,
        lastSeenSeq: null,
      });
    },
    updateCursor: (workspaceId, sessionId, seq) => {
      const key = registryKey(workspaceId);
      const record = sessions.get(key);
      if (record === undefined || record.sessionId !== sessionId) return;
      if (record.lastSeenSeq === null || seq > record.lastSeenSeq) {
        record.lastSeenSeq = seq;
      }
    },
    remove: (workspaceId, sessionId) => {
      const key = registryKey(workspaceId);
      const record = sessions.get(key);
      if (record?.sessionId === sessionId) sessions.delete(key);
    },
  };
})();
