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
import type { AgentProfile } from "../../../types/ipc";
import {
  dom,
  useAgentsPanelDom,
  VOCABULARY_DAEMON,
  makeProfile,
  makeVocabulary,
  storedProfile,
  renderAgentsPanel,
  typeText,
  tickCheckbox,
  makeProvider,
} from "./agentsPanelTestHarness";
import {
  openForm,
  form,
  field,
  nameField,
  noteField,
  modelControl,
  modeControl,
  createButton,
  fillDraft,
} from "./agentsPanelTestQueries";

describe("Settings agents panel — new profile form: create and save", () => {
  useAgentsPanelDom(() => [makeProvider()]);

  it("saves a new profile with enabledForAgents false and an empty id at the end of the list", async () => {
    const beta = makeProfile({ id: "b", name: "Beta" });
    const gamma: AgentProfile = {
      id: "",
      name: "Gamma",
      icon: null,
      note: "Checks the build output.",
      provider: "claude",
      model: "claude-sonnet-4-5",
      modeId: "default",
      thinkingOptionId: null,
      features: {},
      toolOverlay: [],
      enabledForAgents: false,
    };
    await renderAgentsPanel({ profiles: [beta], standingInstructions: "" });
    // The store's read-back after the confirmed create: the daemon minted
    // the id the form could not know.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [beta, { ...gamma, id: "minted-1" }], standingInstructions: "" },
    });
    await openForm();

    await typeText(nameField(), "Gamma");
    await typeText(noteField(), "Checks the build output.");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    // The whole document travels: the old list first, in order, then exactly
    // one new entry. `id` stays empty — the daemon mints it.
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [beta, gamma],
      standingInstructions: "",
    });
  });

  it("passes auto accept into features only when it is ticked, and says what it does", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-backs after each confirmed create: the store grows by
    // one, under the ids the daemon minted.
    vi.mocked(agentProfilesGet)
      .mockResolvedValueOnce({
        document: { profiles: [storedProfile("minted-1")], standingInstructions: "" },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-1"),
            storedProfile("minted-2", { features: { autoAccept: true } }),
          ],
          standingInstructions: "",
        },
      });
    await openForm();

    // The copy must say what the tick does — it is the most consequential
    // control on the form.
    expect(form().textContent).toContain("approve their own permission prompts");

    await fillDraft();
    await act(async () => createButton().click());
    await act(async () => undefined);

    // Unticked: features carries nothing.
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const unticked = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(unticked?.profiles[0]?.features).toEqual({});

    // Again, with the tick: features.autoAccept is the one flag.
    await openForm();
    await fillDraft();
    const autoAccept = field<HTMLInputElement>(
      'input[aria-label="Auto accept for children of this profile"]',
    );
    await tickCheckbox(autoAccept, true);
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(2);
    // The document now carries the first save too; the appended entry is the
    // one this second save created.
    const ticked = vi.mocked(agentProfilesSet).mock.calls[1]?.[0];
    expect(ticked?.profiles.at(-1)?.features).toEqual({ autoAccept: true });
  });

  it("defaults the agents tick to off and saves it only when the human ticks it", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-back after the confirmed create: the tick is stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [storedProfile("minted-1", { enabledForAgents: true })],
        standingInstructions: "",
      },
    });
    await openForm();

    const tick = field<HTMLInputElement>('input[aria-label="Available to agents"]');
    expect(tick.checked).toBe(false);

    await fillDraft();
    await tickCheckbox(tick, true);
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(sent?.profiles[0]?.enabledForAgents).toBe(true);
  });

  it("saves no overlay by default and both peer tools when the tick is on", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-back after the confirmed create.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [storedProfile("minted-1")], standingInstructions: "" },
    });
    await openForm();

    const tick = field<HTMLInputElement>(
      'input[aria-label="Children cannot message peers or create further agents"]',
    );
    expect(tick.checked).toBe(false);
    // The tick names what it denies: peer messages and further creations.
    // It must not promise a surface it does not deliver, so no "design".
    expect(form().textContent).toContain("cannot message other agents or create further");
    expect(form().textContent).not.toContain("Design");

    await fillDraft();
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const unticked = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(unticked?.profiles[0]?.toolOverlay).toEqual([]);

    // Again, with the tick: the overlay denies exactly the two peer tools.
    await openForm();
    await fillDraft();
    const retick = field<HTMLInputElement>(
      'input[aria-label="Children cannot message peers or create further agents"]',
    );
    await tickCheckbox(retick, true);
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(2);
    const ticked = vi.mocked(agentProfilesSet).mock.calls[1]?.[0];
    expect(ticked?.profiles.at(-1)?.toolOverlay).toEqual([
      "devboule_send_message",
      "devboule_create_agent",
    ]);
  });

  it("refuses to save without a model and mode, then saves once both are chosen", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValueOnce(
      makeVocabulary({
        models: {
          state: "present",
          origin: "provider",
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
    // The store's read-back after the (only) confirmed create, at the end.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [storedProfile("scout-1", { name: "Scout", model: "opus", modeId: "code" })],
        standingInstructions: "",
      },
    });
    await openForm();
    await act(async () => undefined);
    await act(async () => undefined);

    // Name only; both selects still on their placeholder.
    await typeText(nameField(), "Scout");
    await act(async () => createButton().click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    expect(dom.container.querySelector('[role="alert"]')?.textContent).toContain("model");

    await typeText(modelControl(), "opus");
    await act(async () => createButton().click());
    await act(async () => undefined);
    expect(agentProfilesSet).not.toHaveBeenCalled();
    expect(dom.container.querySelector('[role="alert"]')?.textContent).toContain("mode");

    await typeText(modeControl(), "code");
    await act(async () => createButton().click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(sent?.profiles[0]?.model).toBe("opus");
    expect(sent?.profiles[0]?.modeId).toBe("code");
  });

  it("keeps the draft on screen under its error when the create is refused", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    vi.mocked(agentProfilesSet).mockRejectedValueOnce({
      code: "io",
      message: "the document holds 65 profiles, over the 64-profile cap",
    });

    await openForm();
    await typeText(nameField(), "Gamma");
    await typeText(noteField(), "Checks the build output.");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    // The refusal is shown, the form still stands, and every field keeps
    // what the human typed into it.
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(dom.container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    expect(nameField().value).toBe("Gamma");
    expect(noteField().value).toBe("Checks the build output.");
    expect((modelControl() as HTMLInputElement).value).toBe("claude-sonnet-4-5");
    expect((modeControl() as HTMLInputElement).value).toBe("default");

    // The same draft is what the retry sends once the daemon takes it — and
    // only confirmation closes the form.
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(2);
    const sent = vi.mocked(agentProfilesSet).mock.calls[1]?.[0];
    expect(sent?.profiles.at(-1)?.name).toBe("Gamma");
    expect(sent?.profiles.at(-1)?.note).toBe("Checks the build output.");
    expect(dom.container.querySelector(".agent-profile-create")).toBeNull();
  });
});
