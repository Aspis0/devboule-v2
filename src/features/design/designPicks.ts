// The picks a person made before a Design session existed (its model, mode and
// effort, remembered per provider) are sent once the session's first manifest
// names the models, before its first prompt, so the first generation runs with
// them. A provider with nothing remembered costs no wait at all.

import { getPreferredMode, getPreferredModel } from "../../lib/agentPrefs";
import type { AgentSessionState } from "../../lib/agentSession";
import { rememberedSwitch } from "../workspace/rememberedPicks";

/** The part of a Design agent session the picks need. */
export interface PickableSession {
  getState(): AgentSessionState;
  subscribe(listener: () => void): () => void;
  setModel(modelId?: string, effort?: string): Promise<void>;
  setMode(modeId: string): Promise<void>;
}

/** How long a new session may take to name its models before the picks give up. */
export const FIRST_MANIFEST_WAIT_MS = 6000;

function manifestOf(session: PickableSession) {
  return session.getState().manifest ?? null;
}

function waitForManifest(session: PickableSession, timeoutMs: number) {
  return new Promise<ReturnType<typeof manifestOf>>((resolve) => {
    const ready = manifestOf(session);
    if (ready !== null) {
      resolve(ready);
      return;
    }
    const unsubscribe = session.subscribe(() => {
      const arrived = manifestOf(session);
      if (arrived === null) return;
      finish(arrived);
    });
    const timer = setTimeout(() => finish(null), timeoutMs);
    function finish(manifest: ReturnType<typeof manifestOf>) {
      clearTimeout(timer);
      unsubscribe();
      resolve(manifest);
    }
  });
}

/** Sends what the person remembered for `providerId` to a session that has just started. */
export async function applyRememberedPicks(
  session: PickableSession,
  providerId: string,
  timeoutMs: number = FIRST_MANIFEST_WAIT_MS,
): Promise<void> {
  if (getPreferredMode(providerId) === null && getPreferredModel(providerId) === null) return;
  const manifest = await waitForManifest(session, timeoutMs);
  if (manifest === null) return;
  const asked = rememberedSwitch(manifest);
  if (asked.mode !== undefined) await session.setMode(asked.mode);
  const modelId = asked.model ?? manifest.currentModelId;
  if (modelId !== undefined && (asked.model !== undefined || asked.effort !== undefined)) {
    await session.setModel(modelId, asked.effort);
  }
}
