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

import {
  agentProfilesGet,
  agentProfilesSet,
  providerVocabularyGet,
  providersList,
} from "../../../lib/tauri";
import type { AgentProfilesDocument, ProviderVocabulary } from "../../../types/ipc";
import {
  useAgentsPanelDom,
  VOCABULARY_DAEMON,
  makeProvider,
  makeVocabulary,
  storedProfile,
  renderAgentsPanel,
  typeText,
} from "./agentsPanelTestHarness";
import {
  openForm,
  openRowEditor,
  form,
  nameField,
  providerField,
  modelControl,
  modeControl,
  createButton,
  selectValues,
} from "./agentsPanelTestQueries";

describe("Settings agents panel — new profile form: provider switch and queries", () => {
  useAgentsPanelDom(() => [makeProvider()]);

  it("never lets a vocabulary reply for the previously selected provider land in the form", async () => {
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [
        makeProvider({ id: "a", executable: "C:\\cli\\a.cmd" }),
        makeProvider({ id: "b", executable: "C:\\cli\\b.cmd" }),
      ],
      unreadableDirs: 0,
    }));
    const resolvers = new Map<string, (reply: ProviderVocabulary) => void>();
    vi.mocked(providerVocabularyGet).mockImplementation((provider: string) => {
      return new Promise<ProviderVocabulary>((resolve) => {
        resolvers.set(provider, resolve);
      });
    });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);

    // The first provider's fetch is in flight when the human switches.
    expect(resolvers.has("a")).toBe(true);
    await typeText(providerField(), "b");
    await act(async () => undefined);
    expect(resolvers.has("b")).toBe(true);

    // The new provider answers.
    await act(async () => {
      resolvers.get("b")?.(
        makeVocabulary({
          provider: "b",
          models: {
            state: "present",
            origin: "provider",
            items: [{ modelId: "b-model", name: "Model B" }],
          },
        }),
      );
    });
    await act(async () => undefined);
    expect(selectValues(modelControl())).toContain("b-model");

    // Now the stale reply for the previous provider arrives.
    await act(async () => {
      resolvers.get("a")?.(
        makeVocabulary({
          provider: "a",
          models: {
            state: "present",
            origin: "provider",
            items: [{ modelId: "a-model", name: "Model A" }],
          },
        }),
      );
    });
    await act(async () => undefined);

    // The form shows provider b; the late answer for a must not have landed.
    expect(selectValues(modelControl())).toContain("b-model");
    expect(selectValues(modelControl())).not.toContain("a-model");
    expect(providerVocabularyGet).toHaveBeenNthCalledWith(1, "a", "", false);
    expect(providerVocabularyGet).toHaveBeenNthCalledWith(2, "b", "", false);
  });

  it("falls back to free text naming the failure when the vocabulary query rejects", async () => {
    vi.mocked(providerVocabularyGet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    // The store's read-back after the confirmed create, at the end.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [storedProfile("minted-1")], standingInstructions: "" },
    });
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // The failure names its own reason — not the provider's "did not
    // publish", not the older-daemon sentence.
    expect(form().textContent).toContain(
      "The vocabulary query failed (A system or file operation failed on this machine.)",
    );
    expect(form().textContent).not.toContain("did not publish");
    expect(form().textContent).not.toContain("older than this app");
    expect(modelControl().tagName).toBe("INPUT");
    expect(modeControl().tagName).toBe("INPUT");

    await typeText(nameField(), "Scout");
    await typeText(modelControl(), "whatever-the-human-knows");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
  });

  it("prefills the ACP mode suggestion labelled as a suggestion when the agent declares no modes", async () => {
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [makeProvider({ id: "zed", protocol: "acp", executable: "C:\\cli\\zed.cmd" })],
      unreadableDirs: 0,
    }));
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        provider: "zed",
        models: {
          state: "present",
          origin: "provider",
          items: [{ modelId: "zed-model", name: "Zed model" }],
        },
      }),
    );
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // Prefilled, and labelled a suggestion — never as something the
    // provider reported.
    expect((modeControl() as HTMLInputElement).value).toBe("default");
    expect(form().textContent).toContain("A suggestion, not something the provider reported");
    // The models axis here is present, so its absent sentence must not show.
    expect(form().textContent).not.toContain("did not publish its models");
  });

  it("saving after a provider switch sends no thinking option and keeps the stored features", async () => {
    // Two installed providers and no `provider_vocabulary` in the handshake:
    // the model and mode are free text, and the switch must still clear
    // everything that belonged to the old provider — and only that: a stored
    // feature key is the profile's own, whichever provider it runs on.
    vi.mocked(providersList).mockResolvedValue({
      providers: [
        {
          id: "grok",
          executable: "C:\\cli\\grok.cmd",
          acpAvailable: true,
          authentication: "ok",
          protocol: "acp",
          origin: "user-binary",
          installed: true,
        },
        {
          id: "claude",
          executable: "C:\\cli\\claude.cmd",
          acpAvailable: false,
          authentication: "ok",
          protocol: "stream-json",
          origin: "user-binary",
          installed: true,
        },
      ],
      unreadableDirs: 0,
    });
    const stored = storedProfile("p-1", {
      name: "Explorer",
      provider: "grok",
      model: "grok-4",
      modeId: "reflect",
      thinkingOptionId: "high",
      features: { autoAccept: true, sandbox: "none" },
    });
    await renderAgentsPanel({ profiles: [stored], standingInstructions: "" });
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          {
            ...stored,
            provider: "claude",
            model: "claude-opus-4-6",
            modeId: "plan",
            thinkingOptionId: null,
            features: { autoAccept: true, sandbox: "none" },
          },
        ],
        standingInstructions: "",
      },
    });

    const editor = await openRowEditor("Explorer");
    const providerSelect = editor.querySelector<HTMLSelectElement>('select[aria-label="Provider"]');
    if (!providerSelect) throw new Error("provider picker did not render");
    await typeText(providerSelect, "claude");
    await act(async () => undefined);

    const model = editor.querySelector<HTMLInputElement>('[aria-label="Model"]');
    const mode = editor.querySelector<HTMLInputElement>('[aria-label="Mode"]');
    const thinking = editor.querySelector<HTMLInputElement>('[aria-label="Thinking option"]');
    if (!model || !mode || !thinking) throw new Error("the cleared fields did not render");
    expect(thinking.value).toBe("");
    await typeText(model, "claude-opus-4-6");
    await typeText(mode, "plan");
    const save = Array.from(editor.querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent === "Save",
    );
    if (!save) throw new Error("the editor's Save button did not render");
    await act(async () => save.click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    expect(sent.profiles[0]?.provider).toBe("claude");
    expect(sent.profiles[0]?.model).toBe("claude-opus-4-6");
    expect(sent.profiles[0]?.modeId).toBe("plan");
    // The old provider's thinking id is gone; the daemon's own tick and the
    // stored key a provider switch never touches both stay.
    expect(sent.profiles[0]?.thinkingOptionId).toBeNull();
    expect(sent.profiles[0]?.features).toEqual({ autoAccept: true, sandbox: "none" });
  });

  it("restores a provider's own fields when the human switches back before saving", async () => {
    vi.mocked(providersList).mockResolvedValue({
      providers: [
        {
          id: "grok",
          executable: "C:\\cli\\grok.cmd",
          acpAvailable: true,
          authentication: "ok",
          protocol: "acp",
          origin: "user-binary",
          installed: true,
        },
        {
          id: "claude",
          executable: "C:\\cli\\claude.cmd",
          acpAvailable: false,
          authentication: "ok",
          protocol: "stream-json",
          origin: "user-binary",
          installed: true,
        },
      ],
      unreadableDirs: 0,
    });
    const stored = storedProfile("p-1", {
      name: "Explorer",
      provider: "grok",
      model: "grok-4",
      modeId: "reflect",
      thinkingOptionId: "high",
      features: { autoAccept: true, sandbox: "none" },
    });
    await renderAgentsPanel({ profiles: [stored], standingInstructions: "" });
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [stored], standingInstructions: "" },
    });

    const editor = await openRowEditor("Explorer");
    const providerSelect = editor.querySelector<HTMLSelectElement>('select[aria-label="Provider"]');
    if (!providerSelect) throw new Error("provider picker did not render");
    const field = (label: string) => {
      const control = editor.querySelector<HTMLInputElement>(`[aria-label="${label}"]`);
      if (!control) throw new Error(`${label} did not render`);
      return control;
    };
    expect(field("Model").value).toBe("grok-4");
    expect(field("Mode").value).toBe("reflect");
    expect(field("Thinking option").value).toBe("high");

    // A wrong pick: the new provider's own vocabulary is empty, not the old
    // provider's — but the stored feature key rides along untouched. It is not
    // drawn: this handshake advertises no `provider_vocabulary`, so nobody has
    // answered what either provider offers, and a form that guessed a control
    // for a key it cannot read would write a boolean over a value only the
    // provider can name. Carried, not shown, and not pruned — the save below
    // proves the carrying.
    await typeText(providerSelect, "claude");
    expect(field("Model").value).toBe("");
    expect(field("Mode").value).toBe("");
    expect(field("Thinking option").value).toBe("");

    // Text typed under the wrong provider is dropped with that provider's
    // own fields; switching back restores what grok held.
    await typeText(field("Model"), "claude-opus-4-6");
    await typeText(providerSelect, "grok");
    expect(field("Model").value).toBe("grok-4");
    expect(field("Mode").value).toBe("reflect");
    expect(field("Thinking option").value).toBe("high");

    const save = Array.from(editor.querySelectorAll<HTMLButtonElement>("button")).find(
      (candidate) => candidate.textContent === "Save",
    );
    if (!save) throw new Error("the editor's Save button did not render");
    await act(async () => save.click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    // The round trip: switch away, type, switch back, save — the stored row
    // is what goes out, stored features included.
    expect(sent.profiles[0]).toEqual(stored);
  });
});
