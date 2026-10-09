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

import { agentProfilesGet, agentProfilesSet } from "../../../lib/tauri";
import type { AgentProfilesDocument } from "../../../types/ipc";
import {
  dom,
  useAgentsPanelDom,
  makeProfile,
  renderAgentsPanel,
  typeText,
} from "./agentsPanelTestHarness";
import { rowButton, panelButton, dialogButton } from "./agentsPanelTestQueries";

describe("Settings agents panel — agents tick, peer restriction and tool denials", () => {
  useAgentsPanelDom(() => []);

  /** The editor's ticks live under Advanced: open the editor, then the section. */
  async function openAdvanced() {
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const advanced = Array.from(
      dom.container.querySelectorAll<HTMLButtonElement>(".edit-card button"),
    ).find((button) => button.textContent === "Advanced");
    if (!advanced) throw new Error("Advanced section did not render");
    await act(async () => advanced.click());
    await act(async () => undefined);
  }

  it("edits the agents tick and the peer restriction from the editor", async () => {
    const restricted = makeProfile({
      enabledForAgents: false,
      toolOverlay: ["devboule_send_message", "devboule_create_agent"],
    });
    await renderAgentsPanel({ profiles: [restricted], standingInstructions: "" });

    await openAdvanced();
    const agentsTick = dom.container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Available to agents"]',
    );
    const peersTick = dom.container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Children cannot message peers or create further agents"]',
    );
    if (!agentsTick || !peersTick) throw new Error("the editor's ticks did not render");
    expect(agentsTick.checked).toBe(false);
    expect(peersTick.checked).toBe(true);
    await act(async () => agentsTick.click());
    await act(async () => peersTick.click());
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument).toEqual({
      profiles: [{ ...restricted, enabledForAgents: true, toolOverlay: [], modelProvider: null }],
      standingInstructions: "",
    });
  });

  it("keeps a peer restriction that shares the overlay with another denial", async () => {
    const guarded = makeProfile({
      toolOverlay: ["devboule_send_message", "devboule_create_agent", "devboule_list_profiles"],
    });
    await renderAgentsPanel({ profiles: [guarded], standingInstructions: "" });

    await openAdvanced();
    // The peer tick is on: the peer tools are in the overlay, whatever else
    // is there with them.
    const peersTick = dom.container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Children cannot message peers or create further agents"]',
    );
    if (!peersTick) throw new Error("the peer tick did not render");
    expect(peersTick.checked).toBe(true);
    // Edit only the note and save: the overlay must survive untouched.
    const noteField = dom.container.querySelector<HTMLTextAreaElement>(
      '.agent-inline-editor textarea[aria-label="Profile note"]',
    );
    if (!noteField) throw new Error("note field did not render");
    await typeText(noteField, "Updated note for the agent.");
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument).toEqual({
      profiles: [
        {
          ...guarded,
          modelProvider: null,
          note: "Updated note for the agent.",
          toolOverlay: ["devboule_send_message", "devboule_create_agent", "devboule_list_profiles"],
        },
      ],
      standingInstructions: "",
    });
  });

  it("carries an older setting's denials through a save untouched", async () => {
    const guarded = makeProfile({
      toolOverlay: ["devboule_send_message", "devboule_create_agent", "devboule_list_profiles"],
    });
    await renderAgentsPanel({ profiles: [guarded], standingInstructions: "" });

    await openAdvanced();
    // One honest line naming the denial, one action — and no per-tool
    // control anywhere.
    expect(dom.container.textContent).toContain("cannot use: devboule_list_profiles");
    expect(dom.container.textContent).not.toContain("Deny a tool by name");
    expect(dom.container.textContent).not.toContain("Add denial");
    expect(dom.container.textContent).not.toContain("Remove denial");
    expect(dom.container.textContent).not.toContain("Other stored tool denials");

    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [{ ...guarded, modelProvider: null }],
      standingInstructions: "",
    });
  });

  it("Allow all tools clears only the non-peer denials", async () => {
    const guarded = makeProfile({
      toolOverlay: ["devboule_send_message", "devboule_create_agent", "devboule_list_profiles"],
    });
    await renderAgentsPanel({ profiles: [guarded], standingInstructions: "" });
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...guarded, toolOverlay: ["devboule_send_message", "devboule_create_agent"] }],
        standingInstructions: "",
      },
    });

    await openAdvanced();
    await act(async () => dialogButton("Allow all tools").click());
    await act(async () => undefined);
    expect(dom.container.textContent).not.toContain("cannot use:");

    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [
        {
          ...guarded,
          modelProvider: null,
          toolOverlay: ["devboule_send_message", "devboule_create_agent"],
        },
      ],
      standingInstructions: "",
    });
  });

  it("keeps stored extras when the peers tick flips on", async () => {
    const guarded = makeProfile({
      toolOverlay: ["devboule_list_profiles"],
    });
    await renderAgentsPanel({ profiles: [guarded], standingInstructions: "" });
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          {
            ...guarded,
            toolOverlay: [
              "devboule_list_profiles",
              "devboule_send_message",
              "devboule_create_agent",
            ],
          },
        ],
        standingInstructions: "",
      },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const advanced = Array.from(
      dom.container.querySelectorAll<HTMLButtonElement>(".edit-card button"),
    ).find((button) => button.textContent === "Advanced");
    if (!advanced) throw new Error("Advanced section did not render");
    await act(async () => advanced.click());
    await act(async () => undefined);
    const peersTick = dom.container.querySelector<HTMLInputElement>(
      '.edit-card input[aria-label="Children cannot message peers or create further agents"]',
    );
    if (!peersTick) throw new Error("peers tick did not render");
    expect(peersTick.checked).toBe(false);
    // Tick on: the pair joins the stored list, the extra stays.
    await act(async () => peersTick.click());
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument).toEqual({
      profiles: [
        {
          ...guarded,
          modelProvider: null,
          toolOverlay: ["devboule_list_profiles", "devboule_send_message", "devboule_create_agent"],
        },
      ],
      standingInstructions: "",
    });
  });
});
