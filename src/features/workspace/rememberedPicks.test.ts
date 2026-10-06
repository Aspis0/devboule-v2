// @vitest-environment happy-dom

// What a new session is asked for on its first manifest: the person's last
// picks for that provider, and nothing the provider does not offer.

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { setPreferredEffort, setPreferredMode, setPreferredModel } from "../../lib/agentPrefs";
import type { SessionManifest } from "../../types/ipc";
import { rememberedSwitch } from "./rememberedPicks";

const STORAGE_KEY = "devboule.agentPrefs";

function manifest(overrides: Partial<SessionManifest> = {}): SessionManifest {
  return {
    type: "session_manifest",
    providerId: "claude",
    currentModelId: "sonnet",
    models: [
      {
        modelId: "sonnet",
        name: "Sonnet",
        currentEffort: "high",
        efforts: [
          { id: "high", label: "High" },
          { id: "low", label: "Low" },
        ],
      },
      { modelId: "opus", name: "Opus", currentEffort: "high", efforts: [] },
    ],
    modes: {
      currentModeId: "default",
      availableModes: [
        { id: "default", name: "Ask before edits" },
        { id: "plan", name: "Plan" },
      ],
    },
    ...overrides,
  };
}

beforeEach(() => {
  localStorage.removeItem(STORAGE_KEY);
});

afterEach(() => {
  localStorage.removeItem(STORAGE_KEY);
});

describe("a provider nobody has picked a mode for", () => {
  it("is asked for nothing, so its own default stands", () => {
    expect(rememberedSwitch(manifest())).toEqual({});
  });

  it("is asked for nothing when the manifest names no provider", () => {
    setPreferredMode("claude", "plan");

    expect(rememberedSwitch(manifest({ providerId: undefined }))).toEqual({});
  });
});

describe("the mode the person picked", () => {
  it("is asked for on the next new agent of that provider", () => {
    setPreferredMode("claude", "plan");

    expect(rememberedSwitch(manifest())).toEqual({ mode: "plan" });
  });

  it("leaves another provider alone", () => {
    setPreferredMode("codex", "full-access");

    expect(rememberedSwitch(manifest())).toEqual({});
  });

  it("falls back to the provider's default when it is no longer offered", () => {
    setPreferredMode("claude", "a-mode-from-another-build");

    expect(rememberedSwitch(manifest())).toEqual({});
  });

  it("is not asked for again when the session already runs in it", () => {
    setPreferredMode("claude", "plan");

    const running = manifest({
      modes: {
        currentModeId: "plan",
        availableModes: [
          { id: "default", name: "Ask before edits" },
          { id: "plan", name: "Plan" },
        ],
      },
    });

    expect(rememberedSwitch(running)).toEqual({});
  });

  it("is skipped when the provider offers no modes at all", () => {
    setPreferredMode("claude", "plan");

    expect(rememberedSwitch(manifest({ modes: undefined }))).toEqual({});
  });
});

describe("the model and effort the person picked", () => {
  it("asks for the model they last used for this provider", () => {
    setPreferredModel("claude", "opus");

    expect(rememberedSwitch(manifest())).toEqual({ model: "opus" });
  });

  it("falls back when the model is gone", () => {
    setPreferredModel("claude", "haiku-4-5");

    expect(rememberedSwitch(manifest())).toEqual({});
  });

  it("asks for the effort they last used on the current model", () => {
    setPreferredEffort("claude", "sonnet", "low");

    expect(rememberedSwitch(manifest())).toEqual({ effort: "low" });
  });

  it("falls back when the effort is gone from the model", () => {
    setPreferredEffort("claude", "sonnet", "extreme");

    expect(rememberedSwitch(manifest())).toEqual({});
  });

  it("carries no effort with a model that declares none", () => {
    setPreferredModel("claude", "opus");
    setPreferredEffort("claude", "opus", "low");

    expect(rememberedSwitch(manifest())).toEqual({ model: "opus" });
  });

  it("does not chase an effort for a model it is not switching to", () => {
    setPreferredEffort("claude", "opus", "low");

    expect(rememberedSwitch(manifest())).toEqual({});
  });

  it("answers with a mode and a model together when both were picked", () => {
    setPreferredMode("claude", "plan");
    setPreferredModel("claude", "opus");

    expect(rememberedSwitch(manifest())).toEqual({ mode: "plan", model: "opus" });
  });
});

describe("storage a machine cannot read", () => {
  it("answers with nothing when the blob is corrupt", () => {
    localStorage.setItem(STORAGE_KEY, "{not json");

    expect(rememberedSwitch(manifest())).toEqual({});
  });
});
