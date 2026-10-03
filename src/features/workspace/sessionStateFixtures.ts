import type { SessionState } from "../../types/ipc";

/** The roster fixtures the surface's tests share. Nothing here is a case, and
 * nothing here is production code. */

export const RECOVERED: SessionState = {
  type: "recovered",
  generation: 1,
  integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
};
