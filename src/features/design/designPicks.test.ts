// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { setPreferredEffort, setPreferredMode, setPreferredModel } from "../../lib/agentPrefs";
import type { AgentSessionState } from "../../lib/agentSession";
import type { SessionManifest } from "../../types/ipc";
import { applyRememberedPicks, type PickableSession } from "./designPicks";

const MANIFEST: SessionManifest = {
  type: "session_manifest",
  providerId: "grok",
  currentModelId: "grok-4",
  models: [
    { modelId: "grok-4", name: "Grok 4" },
    { modelId: "grok-4-mini", name: "Grok 4 Mini", efforts: [{ id: "low", label: "Low" }] },
  ],
  modes: {
    currentModeId: "default",
    availableModes: [
      { id: "default", name: "Default" },
      { id: "plan", name: "Plan" },
    ],
  },
} as SessionManifest;

/** A session whose manifest arrives after `arriveAfterMs` (never, when null). */
function fakeSession(manifest: SessionManifest | null, arriveAfterMs: number | null) {
  let state = { manifest: null } as unknown as AgentSessionState;
  const listeners = new Set<() => void>();
  const session: PickableSession & {
    setMode: ReturnType<typeof vi.fn>;
    setModel: ReturnType<typeof vi.fn>;
  } = {
    getState: () => state,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    setMode: vi.fn(async () => undefined),
    setModel: vi.fn(async () => undefined),
  };
  if (manifest !== null && arriveAfterMs !== null) {
    setTimeout(() => {
      state = { manifest } as unknown as AgentSessionState;
      for (const listener of listeners) listener();
    }, arriveAfterMs);
  }
  return session;
}

beforeEach(() => {
  window.localStorage.clear();
});

describe("applying the picks a person made before the session existed", () => {
  it("sends the remembered mode and model", async () => {
    setPreferredMode("grok", "plan");
    setPreferredModel("grok", "grok-4-mini");
    setPreferredEffort("grok", "grok-4-mini", "low");
    const session = fakeSession(MANIFEST, 0);

    await applyRememberedPicks(session, "grok", 1000);

    expect(session.setMode).toHaveBeenCalledWith("plan");
    // A model switch answers its own effort question (rememberedSwitch), so the effort is not sent with it.
    expect(session.setModel).toHaveBeenCalledWith("grok-4-mini", undefined);
  });

  it("waits for the first manifest, then applies the picks before the first prompt", async () => {
    setPreferredModel("grok", "grok-4-mini");
    const session = fakeSession(MANIFEST, 20);

    await applyRememberedPicks(session, "grok", 1000);

    expect(session.setModel).toHaveBeenCalledWith("grok-4-mini", undefined);
  });

  it("gives up after the wait and sends nothing when no manifest arrives", async () => {
    setPreferredMode("grok", "plan");
    const session = fakeSession(null, null);

    await applyRememberedPicks(session, "grok", 10);

    expect(session.setMode).not.toHaveBeenCalled();
  });

  it("does not wait at all when nothing was remembered for the provider", async () => {
    const session = fakeSession(null, null);
    const started = Date.now();

    await applyRememberedPicks(session, "grok", 5000);

    expect(Date.now() - started).toBeLessThan(1000);
    expect(session.setMode).not.toHaveBeenCalled();
    expect(session.setModel).not.toHaveBeenCalled();
  });
});
