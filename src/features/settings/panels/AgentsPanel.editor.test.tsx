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

import { agentProfilesGet, agentProfilesSet, providersList } from "../../../lib/tauri";
import type { AgentProfile, AgentProfilesDocument } from "../../../types/ipc";
import {
  dom,
  useAgentsPanelDom,
  makeProfile,
  renderAgentsPanel,
  typeText,
} from "./agentsPanelTestHarness";
import { rowButton, panelButton } from "./agentsPanelTestQueries";

describe("Settings agents panel — editing a profile's fields", () => {
  useAgentsPanelDom(() => []);

  it("saves a rename and note while every other field travels untouched", async () => {
    const explorer = makeProfile();
    await renderAgentsPanel({ profiles: [explorer], standingInstructions: "" });
    // The store's read-back after the confirmed save: the rename stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...explorer, name: "Scout", note: "Maps the work before anyone builds." }],
        standingInstructions: "",
      },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editor = dom.container.querySelector(".agent-inline-editor");
    if (!editor) throw new Error("editor did not render");
    const nameField = editor.querySelector<HTMLInputElement>("input");
    const noteField = editor.querySelector<HTMLTextAreaElement>("textarea");
    if (!nameField || !noteField) throw new Error("editor fields did not render");
    await typeText(nameField, "Scout");
    await typeText(noteField, "Maps the work before anyone builds.");

    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [{ ...explorer, name: "Scout", note: "Maps the work before anyone builds." }],
      standingInstructions: "",
    });
  });

  it("edits every field of a profile, the spawn prompt included, and saves them all", async () => {
    const explorer = makeProfile();
    // One installed provider, and no `provider_vocabulary` in the handshake:
    // the model and mode fall back to free text, and the editor must still
    // finish — the same sentence the create form offers.
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
      ],
      unreadableDirs: 0,
    });
    await renderAgentsPanel({ profiles: [explorer], standingInstructions: "" });
    // The store's read-back after the confirmed save, carrying every edit.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          {
            ...explorer,
            name: "Scout",
            note: "Maps the work before anyone builds.",
            spawnPrompt: "Check the diff before you report.",
            model: "grok-4-fast",
            modeId: "reflect",
            thinkingOptionId: "high",
            features: { autoAccept: true },
          },
        ],
        standingInstructions: "",
      },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editor = dom.container.querySelector(".agent-inline-editor");
    if (!editor) throw new Error("editor did not render");
    const pick = <T extends HTMLElement>(selector: string): T => {
      const element = editor.querySelector<T>(selector);
      if (!element) throw new Error(`field ${selector} did not render`);
      return element;
    };
    await typeText(pick<HTMLInputElement>("input"), "Scout");
    await typeText(
      pick<HTMLTextAreaElement>('textarea[aria-label="Profile note"]'),
      "Maps the work before anyone builds.",
    );
    await typeText(
      pick<HTMLTextAreaElement>('textarea[aria-label="Profile spawn prompt"]'),
      "Check the diff before you report.",
    );
    await typeText(pick<HTMLInputElement>('[aria-label="Model"]'), "grok-4-fast");
    await typeText(pick<HTMLInputElement>('[aria-label="Mode"]'), "reflect");
    await typeText(pick<HTMLInputElement>('[aria-label="Thinking option"]'), "high");
    await act(async () =>
      pick<HTMLInputElement>(
        'input[aria-label="Auto accept for children of this profile"]',
      ).click(),
    );

    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [
        {
          ...explorer,
          name: "Scout",
          note: "Maps the work before anyone builds.",
          spawnPrompt: "Check the diff before you report.",
          model: "grok-4-fast",
          modeId: "reflect",
          thinkingOptionId: "high",
          features: { autoAccept: true },
        },
      ],
      standingInstructions: "",
    });
  });

  it("refuses a rename that would put two enabled profiles on one name, before sending", async () => {
    const scout = makeProfile({ id: "s1", name: "Scout", enabledForAgents: true });
    const reviewer = makeProfile({ id: "r1", name: "Reviewer", enabledForAgents: true });
    await renderAgentsPanel({ profiles: [scout, reviewer], standingInstructions: "" });

    await act(async () => rowButton("Reviewer", "Edit").click());
    await act(async () => undefined);
    const nameField = dom.container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Profile name"]',
    );
    if (!nameField) throw new Error("name field did not render");
    await typeText(nameField, "Scout");
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    // Refused before the write: the daemon's rule, named by the form first.
    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = dom.container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("Scout");
    expect(alert?.textContent).toContain("enabled");
  });

  it("saves an icon and clears it to none", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...makeProfile(), icon: "eye" }],
        standingInstructions: "",
      },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const iconField = dom.container.querySelector<HTMLInputElement>('[aria-label="Profile icon"]');
    if (!iconField) throw new Error("icon field did not render");
    await typeText(iconField, "eye");
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument).toEqual({
      profiles: [{ ...makeProfile(), icon: "eye" }],
      standingInstructions: "",
    });

    // Clearing the field is none on the wire: null, never an empty string.
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const iconAgain = dom.container.querySelector<HTMLInputElement>('[aria-label="Profile icon"]');
    if (!iconAgain) throw new Error("icon field did not render on the second open");
    await typeText(iconAgain, "");
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);
    const sent = vi.mocked(agentProfilesSet).mock.calls[1]?.[0] as AgentProfilesDocument;
    expect(sent.profiles[0]?.icon).toBeNull();
  });

  it("keeps a stored feature it cannot draw, and carries it through a save", async () => {
    // The provider was never asked what it offers: this test's mock answers the
    // vocabulary query with no `features` axis at all, which is what a daemon
    // older than the field replies. With no list there is nothing to prune by,
    // so a stored key survives the save — the rule that separates "nobody could
    // ask" from "offers nothing", and the reason the daemon's own prune is
    // conditional in the same way.
    const featured = makeProfile({ features: { autoAccept: true, sandbox: "none" } });
    await renderAgentsPanel({ profiles: [featured], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument).toEqual({
      profiles: [featured],
      standingInstructions: "",
    });
  });

  it("edits and saves a profile that arrived without features or spawn prompt", async () => {
    // The profile exactly as the daemon serves one whose omittable fields are
    // all empty: serde skips `icon`, `spawnPrompt`, `thinkingOptionId`,
    // `features` and `toolOverlay`, and a document older builds wrote never
    // had those keys. Only the keys the daemon always sends are here —
    // reading an omitted one without a default blanked the app live.
    const legacy: AgentProfile = {
      id: "profile-1",
      name: "Explorer",
      note: "Written by an older build.",
      provider: "grok",
      model: "grok-4",
      modeId: "ask",
      enabledForAgents: false,
    };
    await renderAgentsPanel({ profiles: [legacy], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editor = dom.container.querySelector(".agent-inline-editor");
    if (!editor) throw new Error("the editor did not open for the legacy profile");
    // Absent features read as none, not as a crash: the tick is drawn (this
    // daemon answers the vocabulary query without a features axis, so the form
    // falls back to the one feature every agent family applies) and nothing
    // invented is stored for the keys the profile never had.
    expect(
      editor.querySelector('[aria-label="Auto accept for children of this profile"]'),
    ).not.toBeNull();
    expect(editor.querySelector<HTMLInputElement>('[aria-label="Profile name"]')?.value).toBe(
      "Explorer",
    );
    expect(editor.querySelector<HTMLTextAreaElement>('[aria-label="Profile note"]')?.value).toBe(
      "Written by an older build.",
    );

    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    // Every field the profile had survives the save; the omitted ones come
    // back as their empty values, which the daemon skips again on disk.
    expect(sent.profiles[0]).toEqual({
      ...legacy,
      icon: null,
      thinkingOptionId: null,
      features: {},
      toolOverlay: [],
    });
  });
});
