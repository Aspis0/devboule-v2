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

/** A session whose manifest arrives after `arriveAfterMs` (never, when null); a model switch is confirmed a moment after it is asked for. */
function fakeSession(manifest: SessionManifest | null, arriveAfterMs: number | null) {
  let state = { manifest: null } as unknown as AgentSessionState;
  const listeners = new Set<() => void>();
  const notify = () => {
    for (const listener of listeners) listener();
  };
  const session = {
    getState: () => state,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    setMode: vi.fn(async () => undefined),
    setModel: vi.fn(async (modelId?: string) => {
      if (manifest === null || modelId === undefined) return;
      setTimeout(() => {
        state = {
          manifest: { ...manifest, currentModelId: modelId },
        } as unknown as AgentSessionState;
        notify();
      }, 5);
    }),
  } satisfies PickableSession & Record<string, unknown>;
  if (manifest !== null && arriveAfterMs !== null) {
    setTimeout(() => {
      state = { manifest } as unknown as AgentSessionState;
      notify();
    }, arriveAfterMs);
  }
  return session;
}

beforeEach(() => {
  window.localStorage.clear();
});

describe("applying the picks a person made before the session existed", () => {
  it("sends the remembered mode, then the model", async () => {
    setPreferredMode("grok", "plan");
    setPreferredModel("grok", "grok-4-mini");
    const session = fakeSession(MANIFEST, 0);

    await applyRememberedPicks(session, "grok", 1000);

    expect(session.setMode).toHaveBeenCalledWith("plan");
    expect(session.setModel.mock.calls).toEqual([["grok-4-mini"]]);
  });

  it("sends the remembered effort after the remembered model switch is confirmed", async () => {
    setPreferredModel("grok", "grok-4-mini");
    setPreferredEffort("grok", "grok-4-mini", "low");
    const session = fakeSession(MANIFEST, 0);

    await applyRememberedPicks(session, "grok", 1000);

    expect(session.setModel.mock.calls).toEqual([["grok-4-mini"], [undefined, "low"]]);
  });

  it("never sends max to a Claude session, which takes it only at launch", async () => {
    const claude = {
      ...MANIFEST,
      providerId: "claude",
      currentModelId: "m0",
      models: [
        { modelId: "m0", name: "M0" },
        {
          modelId: "m1",
          name: "M1",
          efforts: [
            { id: "low", label: "Low" },
            { id: "max", label: "Max" },
          ],
        },
      ],
    } as SessionManifest;
    setPreferredModel("claude", "m1");
    setPreferredEffort("claude", "m1", "max");
    const session = fakeSession(claude, 0);

    await applyRememberedPicks(session, "claude", 1000);

    expect(session.setModel.mock.calls).toEqual([["m1"]]);
  });

  it("waits for the first manifest, then applies the picks before the first prompt", async () => {
    setPreferredModel("grok", "grok-4-mini");
    const session = fakeSession(MANIFEST, 20);

    await applyRememberedPicks(session, "grok", 1000);

    expect(session.setModel.mock.calls).toEqual([["grok-4-mini"]]);
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
