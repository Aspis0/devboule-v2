// @vitest-environment happy-dom

import { act } from "react";
import { describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const { agentsPanelTauriMock } = await import("./agentsPanelTestMocks");
  return agentsPanelTauriMock(await importOriginal());
});

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

import { agentProfilesSet, providerVocabularyGet } from "../../../lib/tauri";
import {
  useAgentsPanelDom,
  VOCABULARY_DAEMON,
  makeVocabulary,
  renderAgentsPanel,
  makeProvider,
  typeText,
} from "./agentsPanelTestHarness";
import { openForm, form, createButton } from "./agentsPanelTestQueries";

function piVocabulary() {
  return makeVocabulary({
    provider: "pi",
    models: {
      state: "present",
      origin: "provider",
      items: [
        {
          modelId: "mimo-v2-6-flash",
          name: "MiMo V2.6 Flash",
          provider: "opencode-go",
          efforts: [
            { id: "Low", label: "Low" },
            { id: "High", label: "High", default: true },
          ],
        },
        {
          modelId: "nemotron-3-ultra",
          name: "Nemotron 3 Ultra",
          provider: "nvidia",
        },
      ],
    },
    modes: {
      state: "present",
      origin: "daemon",
      items: [
        { id: "ask", name: "Always ask" },
        { id: "bypass", name: "Bypass" },
      ],
    },
    features: {
      state: "present",
      items: [{ id: "autoAccept", label: "Auto accept", author: "daemon", type: "toggle" }],
    },
  });
}

describe("Settings agents panel — profile editor pickers", () => {
  useAgentsPanelDom(() => [makeProvider({ id: "pi" }), makeProvider()]);

  it("offers provider, model, effort and mode as pickers fed by the vocabulary", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(piVocabulary());
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    const editor = form();
    expect(editor.querySelector('select[aria-label="Provider"]')).not.toBeNull();
    const model = editor.querySelector('select[aria-label="Model"]');
    if (!model) throw new Error("model picker did not render");
    expect(model.textContent).toContain("opencode-go");
    expect(editor.querySelector('select[aria-label="Mode"]')).not.toBeNull();
  });

  it("shows only the selected model's effort levels, and no effort picker when the model has none", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(piVocabulary());
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    const editor = form();
    const model = editor.querySelector<HTMLSelectElement>('select[aria-label="Model"]');
    if (!model) throw new Error("model picker did not render");
    // Pick the model that publishes effort levels, by its rendered option.
    const mimoValue = Array.from(model.options).find((option) =>
      option.text.includes("MiMo V2.6 Flash"),
    )?.value;
    if (!mimoValue) throw new Error("mimo model option did not render");
    await act(async () => {
      model.value = mimoValue;
      model.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await act(async () => undefined);
    const effort = editor.querySelector<HTMLSelectElement>('select[aria-label="Effort"]');
    if (!effort) throw new Error("effort picker did not render for a model with levels");
    expect(effort.textContent).toContain("High");
    expect(effort.textContent).not.toContain("Medium");
    // A model with no levels hides the picker entirely.
    const nemoValue = Array.from(model.options).find((option) =>
      option.text.includes("Nemotron 3 Ultra"),
    )?.value;
    if (!nemoValue) throw new Error("nemotron model option did not render");
    await act(async () => {
      model.value = nemoValue;
      model.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await act(async () => undefined);
    expect(editor.querySelector('select[aria-label="Effort"]')).toBeNull();
  });

  it("creates a profile with safe defaults and the provider + model pair stored together", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(piVocabulary());
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    const editor = form();
    const name = editor.querySelector<HTMLInputElement>('input[aria-label="Profile name"]');
    if (!name) throw new Error("name field did not render");
    await typeText(name, "Scout");
    const model = editor.querySelector<HTMLSelectElement>('select[aria-label="Model"]');
    if (!model) throw new Error("model picker did not render");
    const mimoValue = Array.from(model.options).find((option) =>
      option.text.includes("MiMo V2.6 Flash"),
    )?.value;
    if (!mimoValue) throw new Error("mimo model option did not render");
    await act(async () => {
      model.value = mimoValue;
      model.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await act(async () => undefined);
    const mode = editor.querySelector<HTMLSelectElement>('select[aria-label="Mode"]');
    if (!mode) throw new Error("mode picker did not render");
    await act(async () => {
      mode.value = "bypass";
      mode.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await act(async () => createButton().click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]![0];
    const created = sent.profiles[sent.profiles.length - 1]!;
    expect(created.provider).toBe("pi");
    expect(created.model).toBe("mimo-v2-6-flash");
    expect((created as { modelProvider?: string }).modelProvider).toBe("opencode-go");
    // Owner defaults: auto accept on, agents may create on, no peer
    // restriction, idle close at the daemon default (absent key).
    expect(created.enabledForAgents).toBe(true);
    expect(created.features).toMatchObject({ autoAccept: true });
    expect(created.toolOverlay).toEqual([]);
    expect(created).not.toHaveProperty("idleCloseMinutes");
  });

  it("shows disabled pickers with a spinner while pi is being probed, never free text", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(
      makeVocabulary({
        provider: "pi",
        models: { state: "absent", items: [] },
        modes: {
          state: "present",
          origin: "daemon",
          items: [
            { id: "ask", name: "Always ask" },
            { id: "bypass", name: "Bypass" },
          ],
        },
        features: { state: "absent", probing: true, items: [] },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    const editor = form();
    // Never free text while the daemon is still enumerating: both
    // controls wait as disabled pickers with a spinner.
    expect(editor.querySelector('input[aria-label="Model"]')).toBeNull();
    const model = editor.querySelector<HTMLSelectElement>('select[aria-label="Model"]');
    if (!model) throw new Error("waiting model picker did not render");
    expect(model.disabled).toBe(true);
    expect(model.textContent).toContain("Reading pi's model list");
    expect(editor.querySelector('[role="status"]')).not.toBeNull();
    const effort = editor.querySelector<HTMLSelectElement>('select[aria-label="Effort"]');
    if (!effort) throw new Error("waiting effort picker did not render");
    expect(effort.disabled).toBe(true);
    expect(editor.textContent).not.toContain("did not publish its models");
  });

  it("says the read failed when pi's list could not be answered", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(
      makeVocabulary({
        provider: "pi",
        models: { state: "absent", items: [] },
        modes: {
          state: "present",
          origin: "daemon",
          items: [
            { id: "ask", name: "Always ask" },
            { id: "bypass", name: "Bypass" },
          ],
        },
        features: { state: "absent", probing: false, items: [] },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    const editor = form();
    expect(editor.textContent).toContain("could not be read");
    expect(editor.textContent).not.toContain("did not publish its models");
  });

  it("hides effort until a model is chosen", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(piVocabulary());
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    const editor = form();
    // No model chosen yet: neither picker nor free text.
    expect(editor.querySelector('[aria-label="Effort"]')).toBeNull();
  });

  it("keeps Advanced to one-line labels, no helper copy", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(piVocabulary());
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    const editor = form();
    const advancedButton = Array.from(editor.querySelectorAll("button")).find(
      (button) => button.textContent === "Advanced",
    );
    if (!advancedButton) throw new Error("Advanced section did not render");
    await act(async () => advancedButton.click());
    await act(async () => undefined);
    const advanced = editor.querySelector("[data-profile-advanced]");
    if (!advanced) throw new Error("Advanced body did not render");
    expect(advanced.querySelectorAll(".device-field-hint").length).toBe(0);
    expect(advanced.querySelectorAll(".agent-profile-tick-note").length).toBe(0);
    expect(advanced.querySelectorAll("p").length).toBe(0);
  });

  it("keeps everything else under one collapsed Advanced section", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(piVocabulary());
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    const editor = form();
    const advanced = editor.querySelector("[data-profile-advanced]");
    if (!advanced) throw new Error("Advanced section did not render");
    // Collapsed until opened: the safety ticks are not in the main form.
    expect(advanced.textContent).toContain("Advanced");
    expect(editor.textContent).not.toContain("No peer contact");
  });
});
