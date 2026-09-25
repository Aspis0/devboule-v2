// @vitest-environment happy-dom

/**
 * The feature surface of the profile form: what it draws for one provider, and
 * what a save keeps of what the provider no longer offers.
 *
 * The rules these pin, in the order they were won:
 *
 * - one control per feature the provider's answer carries, and no others;
 * - the list is gated by the model, because a feature offered on three models
 *   is not a feature of the fourth;
 * - a stored value the provider no longer offers is dropped on save, silently
 *   (`D4`) — the rule that replaced the read-only rows and their Remove button;
 * - **unless nobody answered**: with no features axis in the reply the form has
 *   no list to prune by, and pruning against an unknown list would delete a
 *   value the daemon has not learned to name. That is the difference between
 *   "offers nothing" and "could not be asked", and the reason the axis is
 *   three-valued;
 * - an ACP provider whose read is still running says so, rather than drawing an
 *   empty form that reads as a provider with nothing.
 */
import { act, createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import type {
  AgentProfile,
  ProviderInfo,
  ProviderVocabulary,
  VocabularyFeature,
} from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  providerVocabularyGet: vi.fn(),
}));

import { providerVocabularyGet } from "../../lib/tauri";
import { AgentProfileForm, VOCABULARY_POLL_MS as POLL_MS } from "./AgentProfileForm";
import {
  offeredFeatures,
  profileFeaturesFromDraft,
  seedFromProfile,
  type ProfileFormSeed,
} from "./AgentProfileDraft";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const TICK: VocabularyFeature = {
  id: "autoAccept",
  label: "Auto accept",
  author: "daemon",
  type: "toggle",
};
const FAST: VocabularyFeature = {
  id: "fastMode",
  label: "Fast",
  author: "daemon",
  type: "toggle",
  models: ["claude-opus-5"],
};
const ENGINE: VocabularyFeature = {
  id: "engine",
  label: "Engine",
  author: "provider",
  type: "select",
  options: [
    { id: "m1", label: "Model one" },
    { id: "m2", label: "Model two" },
  ],
};

const PROVIDERS: ProviderInfo[] = [
  {
    id: "claude",
    executable: "C:\\cli\\claude.cmd",
    acpAvailable: false,
    authentication: "ok",
    protocol: "stream-json",
    origin: "user-binary",
    installed: true,
  },
];

function vocabulary(items: VocabularyFeature[]): ProviderVocabulary {
  return {
    provider: "claude",
    models: { state: "absent", items: [] },
    modes: { state: "absent", items: [] },
    features: { state: "present", items: items },
    source: "probe",
    probedAtMs: null,
  };
}

const EMPTY_SEED: ProfileFormSeed = {
  name: "Explorer",
  icon: "",
  note: "",
  spawnPrompt: "",
  provider: "claude",
  model: "claude-opus-5",
  modeId: "default",
  thinkingOptionId: "",
  features: {},
  overlay: [],
  enabledForAgents: false,
  offeredFeatures: null,
};

function profile(overrides: Partial<AgentProfile> = {}): AgentProfile {
  return {
    id: "p-1",
    name: "Explorer",
    icon: null,
    note: "",
    provider: "claude",
    model: "claude-opus-5",
    modeId: "default",
    thinkingOptionId: null,
    features: {},
    toolOverlay: [],
    enabledForAgents: false,
    ...overrides,
  };
}

function draftOf(overrides: Partial<ProfileFormSeed> = {}): ProfileFormSeed {
  return { ...EMPTY_SEED, ...overrides };
}

describe("the feature rules, without a render", () => {
  /** The drawn list is the provider's answer filtered by the model. Neither a
   *  model name nor a feature name appears in the filter itself. */
  it("offers a gated feature only on the models that carry it", () => {
    const reply = vocabulary([TICK, FAST]);
    expect(offeredFeatures(reply, "claude-opus-5")?.map((f) => f.id)).toEqual([
      "autoAccept",
      "fastMode",
    ]);
    expect(offeredFeatures(reply, "claude-sonnet-5")?.map((f) => f.id)).toEqual(["autoAccept"]);
    // "no model chosen yet" is not "a model that carries it".
    expect(offeredFeatures(reply, "")?.map((f) => f.id)).toEqual(["autoAccept"]);
  });

  /** `[]` and `null` are different facts and only one of them deletes: an
   *  answered list prunes to it, an unanswered question may not consume a
   *  stored key the daemon has never seen named. */
  it("separates an answered list from an unanswered question", () => {
    const stored = { autoAccept: true, sandbox: "none" };
    const draft = draftOf({ features: stored });
    const answered: ProviderVocabulary = {
      ...vocabulary([]),
      features: { state: "none", items: [] },
    };
    // The provider answered "I offer nothing", so each stored key is one it no
    // longer offers and the save writes none of them.
    expect(profileFeaturesFromDraft(draft, offeredFeatures(answered, "claude-opus-5"))).toEqual({});
    // No axis at all: nothing was answered, so nothing may be judged away.
    expect(
      profileFeaturesFromDraft(
        draft,
        offeredFeatures({ ...vocabulary([]), features: undefined }, "x"),
      ),
    ).toEqual(stored);
    const probing: ProviderVocabulary = {
      ...vocabulary([]),
      features: { state: "absent", probing: true, items: [] },
    };
    expect(profileFeaturesFromDraft(draft, offeredFeatures(probing, "x"))).toEqual(stored);
  });

  /** An old profile whose `features` key the daemon skipped still seeds: absent
   *  is the empty map, and reading it as always-present blanked the app live. */
  it("seeds a profile whose features key was never sent", () => {
    const legacy = profile();
    delete (legacy as { features?: unknown }).features;
    delete (legacy as { toolOverlay?: unknown }).toolOverlay;
    delete (legacy as { thinkingOptionId?: unknown }).thinkingOptionId;
    const seeded = seedFromProfile(legacy);
    expect(seeded.features).toEqual({});
    expect(seeded.offeredFeatures).toBeNull();
  });

  /** A stored value whose type no control could produce does not survive the
   *  seed either: the form draws booleans and option ids, and anything else an
   *  older build or a hand-edited file left behind is not a value it can show. */
  it("seeds only the values a control could produce", () => {
    const seeded = seedFromProfile(
      profile({ features: { autoAccept: true, count: 3, nested: { a: 1 }, engine: "m1" } }),
    );
    expect(seeded.features).toEqual({ autoAccept: true, engine: "m1" });
  });
});

describe("the feature controls, as the form draws them", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.useRealTimers();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    vi.mocked(providerVocabularyGet).mockReset();
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  /** Drives a controlled field the way React's own synthetic event does: a
   *  DOM `input` event would not reach `onChange` on a value React owns. */
  async function typeInto(
    field: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement,
    value: string,
  ) {
    const reactKey = Object.keys(field).find((key) => key.startsWith("__reactProps"));
    const props = (field as unknown as Record<string, unknown>)[reactKey ?? ""] as
      | { onChange?: (event: { target: { value: string } }) => void }
      | undefined;
    if (!props?.onChange) throw new Error("the field's onChange did not render");
    await act(async () => {
      props.onChange?.({ target: { value } });
    });
  }

  async function renderForm(seed: ProfileFormSeed) {
    const onCreate = vi.fn();
    await act(async () =>
      root.render(
        createElement(AgentProfileForm, {
          mode: "create" as const,
          seed,
          providers: PROVIDERS,
          catalogLoading: false,
          catalogError: null,
          vocabularySupported: true,
          busy: false,
          onCreate,
          onCancel: vi.fn(),
        }),
      ),
    );
    // The mount's own ask, then its reply: the form clears the answer on a
    // provider change, so a render that has not seen one draws no controls.
    await act(async () => undefined);
    await act(async () => undefined);
    return { onCreate };
  }

  it("draws one control per offered feature, and the kind decides the widget", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(vocabulary([TICK, FAST, ENGINE]));
    await renderForm(draftOf());

    const tick = container.querySelector<HTMLInputElement>(
      '[aria-label="Auto accept for children of this profile"]',
    );
    const fast = container.querySelector<HTMLInputElement>(
      '[aria-label="Profile feature fastMode"]',
    );
    const engine = container.querySelector<HTMLSelectElement>(
      '[aria-label="Profile feature engine"]',
    );
    expect(tick?.getAttribute("type")).toBe("checkbox");
    expect(fast?.getAttribute("type")).toBe("checkbox");
    expect(engine?.tagName).toBe("SELECT");
    // The choices are the agent's, plus the one value the form may add: unset,
    // which stores nothing for the key.
    expect(Array.from(engine?.options ?? []).map((option) => option.value)).toEqual([
      "",
      "m1",
      "m2",
    ]);
  });

  it("redraws the list when the model moves outside a feature's gate", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(vocabulary([TICK, FAST]));
    await renderForm(draftOf());
    expect(container.querySelector('[aria-label="Profile feature fastMode"]')).not.toBeNull();

    const model = container.querySelector<HTMLInputElement>('[aria-label="Model"]');
    if (!model) throw new Error("the model field did not render (absent axes are free text)");
    await typeInto(model, "claude-sonnet-5");
    expect(container.querySelector('[aria-label="Profile feature fastMode"]')).toBeNull();
    // The tick is offered on every model, so a model change never leaves the
    // form without the one control every agent family reads.
    expect(
      container.querySelector('[aria-label="Auto accept for children of this profile"]'),
    ).not.toBeNull();
  });

  it("says it is checking while an ACP provider's list is being read", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue({
      ...vocabulary([]),
      features: { state: "absent", probing: true, items: [] },
    });
    await renderForm(draftOf());
    const status = Array.from(container.querySelectorAll('[role="status"]')).find((element) =>
      element.textContent?.includes("Checking"),
    );
    expect(status).toBeDefined();
    // And it invents no control while it waits: an empty form would read as a
    // provider with nothing, which is the other answer.
    expect(container.querySelector('[aria-label="Profile feature fastMode"]')).toBeNull();
  });
  /// The review's second P1: a `probing` answer is a moment, not a destination.
  /// The form must keep asking until the list arrives, must keep the daemon's
  /// own tick on screen while it waits, and must stop asking once the answer is
  /// there — a form that stopped at the first reply left no feature editable for
  /// the whole life of the editor.
  it("keeps asking while the ACP read runs, and draws the tick while it waits", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const probing = {
      ...vocabulary([]),
      features: { state: "absent" as const, probing: true, items: [] },
    };
    const answered = vocabulary([TICK, ENGINE]);
    vi.mocked(providerVocabularyGet)
      .mockResolvedValueOnce(probing)
      .mockResolvedValueOnce(probing)
      .mockResolvedValue(answered);
    await renderForm(draftOf());
    // First ask: "checking", and the tick present beside it.
    expect(container.textContent).toContain("Checking what this provider offers");
    expect(
      container.querySelector('[aria-label="Auto accept for children of this profile"]'),
    ).not.toBeNull();
    // The poll fires on the interval; advancing time lets it run, and the third
    // reply is the list, so the asking stops there.
    await act(async () => {
      vi.advanceTimersByTimeAsync(POLL_MS + 50);
    });
    await act(async () => {
      vi.advanceTimersByTimeAsync(POLL_MS + 50);
    });
    expect(container.querySelector('[aria-label="Profile feature engine"]')).not.toBeNull();
    expect(container.textContent).not.toContain("Checking what this provider offers");
    // And no further asks: the last reply was final.
    const before = vi.mocked(providerVocabularyGet).mock.calls.length;
    await act(async () => {
      vi.advanceTimersByTimeAsync(POLL_MS * 3);
    });
    expect(vi.mocked(providerVocabularyGet).mock.calls.length).toBe(before);
  });

  /// A read that failed says so, once, and keeps the tick. An empty feature
  /// section would read as "this provider offers nothing", which is the other
  /// answer and a reason for a human to stop looking.
  it("says a failed read failed, and still draws the tick", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue({
      ...vocabulary([]),
      features: { state: "absent", probing: false, items: [] },
    });
    await renderForm(draftOf());
    expect(container.textContent).toContain("could not be asked what it offers");
    expect(
      container.querySelector('[aria-label="Auto accept for children of this profile"]'),
    ).not.toBeNull();
  });

  it("carries a stored key it was never allowed to prune, and draws no row for it", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue({
      ...vocabulary([]),
      features: undefined,
    });
    const { onCreate } = await renderForm(
      draftOf({ features: { autoAccept: true, sandbox: "none" } }),
    );
    expect(
      container.querySelector('[aria-label="Auto accept for children of this profile"]'),
    ).not.toBeNull();
    expect(container.textContent).not.toContain("sandbox");

    const button = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === "Create profile",
    );
    if (!button) throw new Error("the create button did not render");
    await act(async () => button.click());
    const draft = onCreate.mock.calls[0]?.[0] as ProfileFormSeed;
    expect(draft.offeredFeatures).toBeNull();
    // The save the panel would run keeps what the provider never disowned.
    expect(profileFeaturesFromDraft(draft, draft.offeredFeatures)).toEqual({
      autoAccept: true,
      sandbox: "none",
    });
  });

  it("drops a stored key the answered list does not carry, and writes the rest", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(vocabulary([TICK, FAST]));
    const { onCreate } = await renderForm(
      draftOf({ features: { autoAccept: true, fastMode: true, sandbox: "none" } }),
    );
    const button = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === "Create profile",
    );
    if (!button) throw new Error("the create button did not render");
    await act(async () => button.click());
    const draft = onCreate.mock.calls[0]?.[0] as ProfileFormSeed;
    expect(draft.offeredFeatures?.map((feature) => feature.id)).toEqual(["autoAccept", "fastMode"]);
    // `sandbox` had no control, so no value of it was ever written: the key
    // leaves the profile on save, silently — D4, and the reason the read-only
    // row with its Remove button is gone.
    expect(profileFeaturesFromDraft(draft, draft.offeredFeatures)).toEqual({
      autoAccept: true,
      fastMode: true,
    });
  });
});
