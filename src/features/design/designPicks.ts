// The picks a person made before a Design session existed (its model, mode and
// effort, remembered per provider) are sent once the session's first manifest
// names the models, before its first prompt, so the first generation runs with
// them. A provider with nothing remembered costs no wait at all.

import { getPreferredEffort, getPreferredMode, getPreferredModel } from "../../lib/agentPrefs";
import type { AgentSessionState } from "../../lib/agentSession";
import { liveEffortOptions } from "../workspace/liveEffortOptions";
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

/** Waits until the session's manifest names `modelId` as current (the switch is confirmed). */
function waitForCurrentModel(session: PickableSession, modelId: string, timeoutMs: number) {
  return new Promise<void>((resolve) => {
    const confirmed = () => session.getState().manifest?.currentModelId === modelId;
    if (confirmed()) {
      resolve();
      return;
    }
    const unsubscribe = session.subscribe(() => {
      if (!confirmed()) return;
      finish();
    });
    const timer = setTimeout(finish, timeoutMs);
    function finish() {
      clearTimeout(timer);
      unsubscribe();
      resolve();
    }
  });
}

/**
 * Sends what the person remembered for `providerId` to a session that has just
 * started: the mode, then the model, and the effort for that model once the
 * model switch is confirmed (the daemon answers a model switch first, and an
 * effort sent with it would be asked for the old model). A `max` effort is not
 * sent to Claude, which takes it only at launch.
 */
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
  const switchedTo = asked.model;
  if (switchedTo !== undefined) {
    await session.setModel(switchedTo);
    await waitForCurrentModel(session, switchedTo, timeoutMs);
  }
  const target = switchedTo ?? manifest.currentModelId;
  if (target === undefined) return;
  const remembered = getPreferredEffort(providerId, target);
  const offered = manifest.models.find((model) => model.modelId === target)?.efforts ?? [];
  const effort = liveEffortOptions(offered, providerId, undefined).find(
    (option) => option.id === remembered,
  );
  if (effort !== undefined) await session.setModel(undefined, effort.id);
}
