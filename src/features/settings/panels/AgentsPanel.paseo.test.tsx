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

import { agentProfilesGet, providerVocabularyGet } from "../../../lib/tauri";
import type { AgentProfile } from "../../../types/ipc";
import {
  dom,
  useAgentsPanelDom,
  VOCABULARY_DAEMON,
  makeVocabulary,
  storedProfile,
  renderAgentsPanel,
  makeProvider,
} from "./agentsPanelTestHarness";

describe("Settings agents panel — Paseo-style rows", () => {
  useAgentsPanelDom(() => [makeProvider()]);

  function piProfile(): AgentProfile {
    return {
      ...storedProfile("p-pi", {
        name: "Scout",
        provider: "pi",
        model: "mimo-v2-6-flash",
        modeId: "bypass",
        thinkingOptionId: "High",
        note: "Fast router for scouting tasks.",
        enabledForAgents: true,
      }),
      // The serving provider stored beside the bare model id.
      modelProvider: "opencode-go",
    } as AgentProfile;
  }

  it("lists profiles as icon, name, provider · model · effort, one-line note, icon actions", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(makeVocabulary());
    await renderAgentsPanel(
      { profiles: [piProfile()], standingInstructions: "" },
      VOCABULARY_DAEMON,
    );
    await act(async () => undefined);
    const row = dom.container.querySelector(".agent-profile-row");
    if (!row) throw new Error("profile row did not render");
    // Provider · serving provider · model · effort, one line.
    expect(row.textContent).toContain("pi · opencode-go · mimo-v2-6-flash · High");
    expect(row.textContent).toContain("Fast router for scouting tasks.");
    // Icon actions only: up, down, edit, delete. No tick on the row.
    expect(row.querySelector('button[aria-label="Move Scout up"]')).not.toBeNull();
    expect(row.querySelector('button[aria-label="Move Scout down"]')).not.toBeNull();
    expect(row.querySelector('button[aria-label="Edit Scout"]')).not.toBeNull();
    expect(row.querySelector('button[aria-label="Delete Scout"]')).not.toBeNull();
    expect(row.querySelector('input[type="checkbox"]')).toBeNull();
  });

  it("shows standing instructions as one row with an Edit button, no inline textarea", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(makeVocabulary());
    await renderAgentsPanel({ profiles: [], standingInstructions: "Be brief." }, VOCABULARY_DAEMON);
    await act(async () => undefined);
    const panel = dom.container.querySelector("#settings-panel-agents");
    if (!panel) throw new Error("agents panel did not render");
    expect(panel.textContent).toContain("Standing instructions");
    // No inline editor: the textarea only exists inside the opened editor.
    expect(
      panel.querySelector('textarea[aria-label="Standing instructions for every agent"]'),
    ).toBeNull();
    const edit = Array.from(panel.querySelectorAll("button")).find(
      (button) => button.textContent === "Edit" && button.closest("[data-settings-row]") !== null,
    );
    if (!edit) throw new Error("standing instructions Edit row did not render");
    await act(async () => edit.click());
    expect(
      dom.container.querySelector('textarea[aria-label="Standing instructions for every agent"]'),
    ).not.toBeNull();
  });

  it("renders the delegation switch as one toggle row with a one-line label", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(makeVocabulary());
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, [
      ...VOCABULARY_DAEMON,
      "permission_delegation",
    ]);
    await act(async () => undefined);
    const toggle = dom.container.querySelector(
      'input[aria-label="Let agents answer their children\'s cards"]',
    );
    if (!toggle) throw new Error("delegation toggle row did not render");
    expect(toggle.getAttribute("role")).toBe("switch");
    // No explanatory paragraph beside it.
    expect(toggle.closest("[data-settings-row]")?.querySelectorAll("p").length).toBe(0);
  });

  it("shows no warning box when no profile is enabled for agents", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(makeVocabulary());
    await renderAgentsPanel(
      { profiles: [storedProfile("p-1", { enabledForAgents: false })], standingInstructions: "" },
      VOCABULARY_DAEMON,
    );
    await act(async () => undefined);
    expect(dom.container.querySelector(".agent-profiles-off")).toBeNull();
    expect(dom.container.textContent).not.toContain("No profile is ticked");
  });

  it("shows no byte counters before approaching a cap", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(makeVocabulary());
    await renderAgentsPanel({ profiles: [], standingInstructions: "Be brief." }, VOCABULARY_DAEMON);
    await act(async () => undefined);
    expect(dom.container.textContent).not.toContain("bytes");
  });

  it("labels the list with an Agent profiles section and a + action", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(makeVocabulary());
    await renderAgentsPanel({ profiles: [], standingInstructions: "" }, VOCABULARY_DAEMON);
    await act(async () => undefined);
    const section = dom.container.querySelector("[data-settings-section]");
    if (!section) throw new Error("agent profiles section did not render");
    expect(section.textContent).toContain("Agent profiles");
    expect(section.querySelector('button[aria-label="New profile"]')).not.toBeNull();
  });

  it("keeps reading old profiles that store a bare model id", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue(
      makeVocabulary({
        models: {
          state: "present",
          origin: "provider",
          items: [{ modelId: "mimo-v2-6-flash", name: "MiMo V2.6 Flash" }],
        },
      }),
    );
    const bare = storedProfile("p-bare", { provider: "pi", model: "mimo-v2-6-flash" });
    await renderAgentsPanel({ profiles: [bare], standingInstructions: "" }, VOCABULARY_DAEMON);
    await act(async () => undefined);
    const row = dom.container.querySelector(".agent-profile-row");
    if (!row) throw new Error("profile row did not render");
    expect(row.textContent).toContain("pi · mimo-v2-6-flash");
    expect(vi.mocked(agentProfilesGet)).toHaveBeenCalled();
  });
});
