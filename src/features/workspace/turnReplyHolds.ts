// 30s bounds missed finish events and replies that arrive after their turn ended.
const TURN_REPLY_HOLD_MS = 30_000;

interface Hold {
  sawWorking: boolean;
  timer: ReturnType<typeof setTimeout>;
}

export function createTurnReplyHolds(onRelease: () => void) {
  const pending = new Set<string>();
  const holds = new Map<string, Hold>();

  function release(id: string): boolean {
    const hold = holds.get(id);
    if (hold === undefined) return false;
    clearTimeout(hold.timer);
    holds.delete(id);
    onRelease();
    return true;
  }

  function clear(): boolean {
    let changed = false;
    for (const id of [...holds.keys()]) changed = release(id) || changed;
    return changed;
  }

  return {
    begin(id: string): void {
      pending.add(id);
    },

    settle(id: string, turnActive: boolean | undefined, rosterBusy = false): boolean {
      const wasPending = pending.delete(id);
      if (!wasPending || turnActive !== true) return false;
      const timer = setTimeout(() => release(id), TURN_REPLY_HOLD_MS);
      holds.set(id, { sawWorking: rosterBusy, timer });
      return true;
    },

    agentFinished(): boolean {
      return clear();
    },

    invalidate(): boolean {
      return clear();
    },

    discard(): void {
      pending.clear();
      clear();
    },

    observeActivity(activity: "working" | "blocked" | "idle" | "unknown"): boolean {
      if (activity === "working" || activity === "blocked") {
        for (const hold of holds.values()) hold.sawWorking = true;
        return false;
      }
      if (activity !== "idle") return false;
      let changed = false;
      for (const [id, hold] of holds) {
        if (hold.sawWorking) changed = release(id) || changed;
      }
      return changed;
    },

    hasHolds(): boolean {
      return holds.size > 0;
    },

    hasPending(): boolean {
      return pending.size > 0;
    },
  };
}
