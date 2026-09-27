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

import { agentProfilesGet, agentProfilesSet, providerVocabularyGet } from "../../../lib/tauri";
import type { AgentProfilesDocument, ProviderVocabulary } from "../../../types/ipc";
import {
  dom,
  useAgentsPanelDom,
  VOCABULARY_DAEMON,
  makeVocabulary,
  storedProfile,
  renderAgentsPanel,
  makeProvider,
} from "./agentsPanelTestHarness";
import {
  openForm,
  openRowEditor,
  form,
  modelControl,
  modeControl,
  createButton,
  selectValues,
  fillDraft,
} from "./agentsPanelTestQueries";

describe("Settings agents panel — new profile form: vocabulary states", () => {
  useAgentsPanelDom(() => [makeProvider()]);

  it("renders absent vocabulary as free text with the spec's sentence, never as a select", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(makeVocabulary());
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    expect(providerVocabularyGet).toHaveBeenCalledWith("claude", "", false);
    // The spec's own sentence, once per axis.
    expect(form().textContent).toContain(
      "This provider did not publish its models; what you type is checked when the session starts.",
    );
    expect(form().textContent).toContain(
      "This provider did not publish its modes; what you type is checked when the session starts.",
    );
    // Free text, not an empty select: the human can finish the form.
    expect(modelControl().tagName).toBe("INPUT");
    expect(modeControl().tagName).toBe("INPUT");
  });

  it("shows a stored model and mode the provider no longer lists, instead of an empty field", async () => {
    // The reply publishes one model and one mode, neither of them the row's.
    // A select over published items alone would render both fields empty,
    // hiding the values the human opened the editor to change; the stored
    // value is appended and labelled as the saved one, so the row's value is
    // visible, selected, and kept by a save that touches nothing else.
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: {
          state: "present",
          origin: "provider",
          items: [{ modelId: "claude-opus-4-6", name: "Claude Opus 4.6" }],
        },
        modes: { state: "present", origin: "provider", items: [{ id: "plan", name: "Plan" }] },
      }),
    );
    const stored = storedProfile("p-1", { name: "Explorer" });
    await renderAgentsPanel({ profiles: [stored], standingInstructions: "" }, VOCABULARY_DAEMON);
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [stored], standingInstructions: "" },
    });
    const editor = await openRowEditor("Explorer");
    await act(async () => undefined);

    const model = editor.querySelector<HTMLSelectElement>('select[aria-label="Model"]');
    const mode = editor.querySelector<HTMLSelectElement>('select[aria-label="Mode"]');
    if (!model || !mode) throw new Error("the model and mode selects did not render");
    expect(selectValues(model)).toEqual(["", "claude-opus-4-6", "claude-sonnet-4-5"]);
    expect(selectValues(mode)).toEqual(["", "plan", "default"]);
    expect(model.value).toBe("claude-sonnet-4-5");
    expect(mode.value).toBe("default");
    expect(
      Array.from(model.options).find((option) => option.value === "claude-sonnet-4-5")?.textContent,
    ).toBe("claude-sonnet-4-5 (the value saved on this profile)");

    // A save that changes no vocabulary field carries the stored pair, not
    // the empty string a blank select would have left in the draft.
    const save = Array.from(editor.querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent === "Save",
    );
    if (!save) throw new Error("the editor's Save button did not render");
    await act(async () => save.click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    expect(sent.profiles[0]?.model).toBe("claude-sonnet-4-5");
    expect(sent.profiles[0]?.modeId).toBe("default");
  });

  it("says none and absent differently: a provider that answers 'I have none' is not a silent one", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({ models: { state: "none", items: [] } }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // `none`: the provider CAN answer and answered "I have none".
    expect(form().textContent).toContain("This provider reports no models");
    expect(form().textContent).not.toContain("did not publish its models");
    // The modes axis in the same reply is `absent`: the two sentences must
    // not collapse into one.
    expect(form().textContent).toContain("did not publish its modes");
    expect(form().textContent).not.toContain("reports no modes");
    // Both fields stay required and typeable either way.
    expect(modelControl().tagName).toBe("INPUT");
    expect(modeControl().tagName).toBe("INPUT");
  });

  it("keeps the form completable on an older daemon, names that reason, and sends no vocabulary query", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-back after the confirmed create.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [storedProfile("minted-1")], standingInstructions: "" },
    });
    await openForm();

    // The older-daemon sentence — not the provider's "did not publish".
    expect(form().textContent).toContain("older than this app");
    expect(form().textContent).not.toContain("did not publish");
    expect(providerVocabularyGet).not.toHaveBeenCalled();
    // Free text on both axes: the human can complete and save.
    expect(modelControl().tagName).toBe("INPUT");
    expect(modeControl().tagName).toBe("INPUT");
    await fillDraft();
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(sent?.profiles[0]?.model).toBe("claude-sonnet-4-5");
    expect(sent?.profiles[0]?.modeId).toBe("default");
  });

  it("shows the honest sentence for origin daemon exactly once, and not for origin provider", async () => {
    // Models are the daemon's own mapping; modes are the provider's own
    // answer. The honest sentence belongs to the first, only.
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: {
          state: "present",
          origin: "daemon",
          items: [{ modelId: "opus", name: "Opus" }],
        },
        modes: {
          state: "present",
          origin: "provider",
          items: [{ id: "code", name: "Code" }],
        },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    expect(modelControl().tagName).toBe("SELECT");
    expect(modeControl().tagName).toBe("SELECT");
    const mentions =
      dom.container.textContent?.match(/not something the provider published/g) ?? [];
    expect(mentions).toHaveLength(1);
    // The published items are offered as they arrived.
    expect(selectValues(modelControl())).toContain("opus");
    expect(selectValues(modeControl())).toContain("code");
  });

  it("names a malformed reply and keeps its usable models list instead of calling it a failed query", async () => {
    // No `modes` axis at all — a malformed reply; models arrived intact.
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce({
      provider: "claude",
      models: {
        state: "present",
        origin: "provider",
        items: [{ modelId: "opus", name: "Opus" }],
      },
      source: "probe",
    } as unknown as ProviderVocabulary);
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // The usable half is kept: a select over the models that arrived.
    expect(modelControl().tagName).toBe("SELECT");
    expect(selectValues(modelControl())).toContain("opus");
    // The missing half is named as its own state — malformed, which is not
    // a failed query and not `absent`.
    expect(modeControl().tagName).toBe("INPUT");
    expect(form().textContent).toContain("reply was malformed");
    expect(form().textContent).toContain("carried no modes axis");
    expect(form().textContent).not.toContain("The vocabulary query failed");
    expect(form().textContent).not.toContain("did not publish its modes");
    // And the malformed half did not take the models sentence with it.
    expect(form().textContent).not.toContain("carried no models axis");
  });

  it("names the contradiction when present arrives with an empty list, instead of an empty select", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: { state: "present", origin: "provider", items: [] },
        modes: { state: "absent", items: [] },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // `present` with no items is the reply the spec forbids: the form names
    // the contradiction and stays typeable rather than rendering a select
    // with nothing to select.
    expect(modelControl().tagName).toBe("INPUT");
    expect(form().textContent).toContain("listed none — a contradiction");
    expect(form().textContent).not.toContain("reports no models");
    expect(form().textContent).not.toContain("did not publish its models");
    // The modes axis in the same reply is a real `absent`.
    expect(modeControl().tagName).toBe("INPUT");
    expect(form().textContent).toContain("did not publish its modes");
  });

  it("names a state value it does not know instead of rendering a silent free-text field", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: { state: "expired", items: [] } as unknown as ProviderVocabulary["models"],
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    expect(modelControl().tagName).toBe("INPUT");
    // The unknown value is shown as received, and none of the known states
    // is claimed for it — an unexplained field is the one dishonest answer.
    expect(form().textContent).toContain('a value this app does not know ("expired")');
    expect(form().textContent).not.toContain("did not publish its models");
    expect(form().textContent).not.toContain("reports no models");
    expect(form().textContent).not.toContain("reply was malformed");
  });

  it("renders a present list whose origin is undeclared, and says no author is declared", async () => {
    // `origin` omitted entirely — the type allows it at runtime, the spec
    // conditions it on `present`, and the daemon side will enforce it; this
    // is the side where an undeclared list must not read as a declared one.
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: { state: "present", items: [{ modelId: "opus", name: "Opus" }] },
        modes: { state: "present", items: [{ id: "code", name: "Code" }] },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // The items themselves are usable: a select, not free text.
    expect(modelControl().tagName).toBe("SELECT");
    expect(selectValues(modelControl())).toContain("opus");
    // The missing authorship is named on both axes. No sentence at all is
    // what the eye reads as "the provider published this" — the stronger
    // of the two authorships.
    expect(form().textContent).toContain("no author declared");
    expect(form().textContent).toContain("whether the provider published it");
    expect(form().textContent).not.toContain("not something the provider published");
  });
});
