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
import { dom, useAgentsPanelDom, makeProfile, renderAgentsPanel } from "./agentsPanelTestHarness";
import {
  profileRows,
  rowByName,
  tickBox,
  rowButton,
  panelButton,
  dialogButton,
} from "./agentsPanelTestQueries";

describe("Settings agents panel — profile rows and their actions", () => {
  useAgentsPanelDom(() => []);

  it("ticking agents-may-create writes exactly that flag and nothing else", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed write: the tick is stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...makeProfile(), enabledForAgents: true }],
        standingInstructions: "",
      },
    });

    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    // Deep equality on the whole document: provider, model, mode, features,
    // note, order and the standing instructions travel untouched — only the
    // tick flipped.
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [{ ...makeProfile(), enabledForAgents: true }],
      standingInstructions: "",
    });
    expect(tickBox("Explorer").checked).toBe(true);
  });

  it("reverts the tick and shows the daemon sentence verbatim on a failed write", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    vi.mocked(agentProfilesSet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });

    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    expect(tickBox("Explorer").checked).toBe(false);
    expect(dom.container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
  });

  it("shows the off-switch sentence once the last ticked profile is untoggled", async () => {
    await renderAgentsPanel({
      profiles: [
        makeProfile({ enabledForAgents: true }),
        makeProfile({ id: "profile-2", name: "Coder" }),
      ],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed write: both rows, untoggled.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [makeProfile(), makeProfile({ id: "profile-2", name: "Coder" })],
        standingInstructions: "",
      },
    });

    // One ticked: the door is open, the sentence must be absent.
    expect(dom.container.textContent).not.toContain("agents cannot start agents");

    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    // The last tick is gone, so the section reads as the off switch it is.
    expect(dom.container.textContent).toContain("agents cannot start agents");
    expect(tickBox("Explorer").checked).toBe(false);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
  });

  it("moving a profile up sends the reordered document and re-renders in that order", async () => {
    const beta = makeProfile({ id: "b", name: "Beta" });
    const alpha = makeProfile({ id: "a", name: "Alpha" });
    await renderAgentsPanel({ profiles: [beta, alpha], standingInstructions: "" });
    // The store's read-back after the confirmed write: the new order.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [alpha, beta], standingInstructions: "" },
    });

    const up = rowByName("Alpha").querySelector<HTMLButtonElement>(
      "button[aria-label='Move Alpha up']",
    );
    if (!up) throw new Error("move-up button did not render");
    await act(async () => up.click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [alpha, beta],
      standingInstructions: "",
    });
    expect(profileRows().map((row) => row.querySelector(".profile-name")?.textContent)).toEqual([
      "Alpha",
      "Beta",
    ]);
  });

  it("deletes only the armed profile, and only after the inline confirm", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile(), makeProfile({ id: "profile-2", name: "Coder" })],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed delete: one row left.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [makeProfile({ id: "profile-2", name: "Coder" })],
        standingInstructions: "",
      },
    });

    // The first click arms the row and sends nothing.
    await act(async () => rowButton("Explorer", "Delete").click());
    await act(async () => undefined);
    expect(agentProfilesSet).not.toHaveBeenCalled();
    expect(dom.container.textContent).toContain("Deletes this profile");

    await act(async () => panelButton("Delete now").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0];
    expect(sent?.profiles.map((profile) => profile.name)).toEqual(["Coder"]);
    expect(profileRows().map((row) => row.querySelector(".profile-name")?.textContent)).toEqual([
      "Coder",
    ]);
  });

  it("shows the tile, the thinking meta and the spawn prompt on the row", async () => {
    await renderAgentsPanel({
      profiles: [
        makeProfile({
          name: "Coder",
          icon: "✦",
          thinkingOptionId: "high",
          spawnPrompt: "Work in small steps. Run the tests after every change.",
        }),
        makeProfile({ id: "profile-2", name: "Plain" }),
      ],
      standingInstructions: "",
    });

    const coder = rowByName("Coder");
    expect(coder.querySelector(".profile-tile")?.textContent).toBe("✦");
    expect(coder.querySelector(".profile-meta")?.textContent).toBe(
      "grok · grok-4 · ask · high thinking",
    );
    expect(coder.querySelector(".profile-spawn")?.textContent).toContain("Work in small steps.");

    // No icon, no thinking option, no spawn prompt: the letter tile, a meta
    // line that claims nothing about thinking, and the named absence.
    const plain = rowByName("Plain");
    expect(plain.querySelector(".profile-tile")?.textContent).toBe("P");
    expect(plain.querySelector(".profile-meta")?.textContent).toBe("grok · grok-4 · ask");
    expect(plain.querySelector(".profile-spawn")?.textContent).toContain("No spawn prompt");
  });

  it("dims the dead reorder ends", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "b", name: "Beta" }), makeProfile({ id: "a", name: "Alpha" })],
      standingInstructions: "",
    });

    const firstUp = rowByName("Beta").querySelector<HTMLButtonElement>(
      "button[aria-label='Move Beta up']",
    );
    const lastDown = rowByName("Alpha").querySelector<HTMLButtonElement>(
      "button[aria-label='Move Alpha down']",
    );
    expect(firstUp?.disabled).toBe(true);
    expect(firstUp?.classList.contains("profile-is-dim")).toBe(true);
    expect(lastDown?.disabled).toBe(true);
    expect(lastDown?.classList.contains("profile-is-dim")).toBe(true);
    const liveDown = rowByName("Beta").querySelector<HTMLButtonElement>(
      "button[aria-label='Move Beta down']",
    );
    expect(liveDown?.disabled).toBe(false);
    expect(liveDown?.classList.contains("profile-is-dim")).toBe(false);
  });

  it("orders the page: behaviour card, profile list, delegation last", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    const panel = dom.container.querySelector("#settings-panel-agents");
    if (!panel) throw new Error("agents panel did not render");
    const order = Array.from(
      panel.querySelectorAll(
        ".agent-standing, .agent-profile-list, .agent-delegation, .agent-delegation-unavailable",
      ),
    );
    expect(order.map((el) => el.className)).toEqual([
      expect.stringContaining("agent-standing"),
      expect.stringContaining("agent-profile-list"),
      expect.stringMatching(/agent-delegation(-unavailable)?/),
    ]);
  });

  it("shows one empty-state line with the New profile action when no profile exists", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });

    expect(profileRows()).toHaveLength(0);
    const empty = dom.container.querySelector(".agent-profile-empty");
    if (!empty) throw new Error("empty state did not render");
    expect(empty.textContent).toContain("No profiles yet");
    const action = empty.querySelector<HTMLButtonElement>("button");
    expect(action?.textContent).toBe("New profile");
    expect(action?.disabled).toBe(false);
    // One New profile action on the page, beside the line — not a second
    // one below it.
    const actions = Array.from(dom.container.querySelectorAll<HTMLButtonElement>("button")).filter(
      (candidate) => candidate.textContent === "New profile",
    );
    expect(actions).toHaveLength(1);
  });

  it("lands delete focus on the previous pencil when the last row goes", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "a", name: "Alpha" }), makeProfile({ id: "b", name: "Beta" })],
      standingInstructions: "",
    });
    vi.mocked(agentProfilesGet).mockResolvedValue({
      document: { profiles: [makeProfile({ id: "a", name: "Alpha" })], standingInstructions: "" },
    });

    await act(async () => {
      const trash = Array.from(
        rowByName("Beta").querySelectorAll<HTMLButtonElement>("button"),
      ).find((candidate) => candidate.getAttribute("aria-label") === "Delete Beta");
      if (!trash) throw new Error("row trash button did not render");
      await trash.click();
    });
    await act(async () => undefined);
    await act(async () => panelButton("Delete now").click());
    await act(async () => undefined);
    await act(async () => undefined);
    // The deleted index is past the end now: the clamp lands previous.
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Edit Alpha");
  });

  it("lands delete focus on the next row's pencil, the previous at the end", async () => {
    await renderAgentsPanel({
      profiles: [
        makeProfile({ id: "a", name: "Alpha" }),
        makeProfile({ id: "b", name: "Beta" }),
        makeProfile({ id: "c", name: "Coder" }),
      ],
      standingInstructions: "",
    });
    vi.mocked(agentProfilesGet).mockResolvedValue({
      document: {
        profiles: [makeProfile({ id: "b", name: "Beta" })],
        standingInstructions: "",
      },
    });

    await act(async () => rowButton("Alpha", "Delete").click());
    await act(async () => undefined);
    await act(async () => panelButton("Delete now").click());
    await act(async () => undefined);
    await act(async () => undefined);
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Edit Beta");
  });

  it("lands delete focus on the trash when refused, even with a hostile name", async () => {
    const hostile = 'Bob "the \\ builder';
    await renderAgentsPanel({
      profiles: [makeProfile({ name: hostile })],
      standingInstructions: "",
    });
    vi.mocked(agentProfilesSet).mockRejectedValueOnce({
      code: "io",
      message: "profile file unwritable",
    });

    await act(async () => {
      // Found by comparing labels, never by interpolating the hostile
      // name into a selector — the production code must do the same.
      const trash = Array.from(
        rowByName(hostile).querySelectorAll<HTMLButtonElement>("button"),
      ).find((candidate) => candidate.getAttribute("aria-label") === `Delete ${hostile}`);
      if (!trash) throw new Error("row trash button did not render");
      await trash.click();
    });
    await act(async () => undefined);
    await act(async () => panelButton("Delete now").click());
    await act(async () => undefined);
    await act(async () => undefined);
    // No throw, no boundary: focus is back on the arming trash.
    expect(document.activeElement?.getAttribute("aria-label")).toBe(`Delete ${hostile}`);
  });

  it("lands delete focus on the list when nothing is left, and on the trash when refused", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    vi.mocked(agentProfilesSet).mockRejectedValueOnce({
      code: "io",
      message: "profile file unwritable",
    });

    await act(async () => rowButton("Explorer", "Delete").click());
    await act(async () => undefined);
    await act(async () => panelButton("Delete now").click());
    await act(async () => undefined);
    await act(async () => undefined);
    // Refused: the row is back, focus on the trash that armed it.
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Delete Explorer");

    vi.mocked(agentProfilesSet).mockImplementation(async () => undefined);
    vi.mocked(agentProfilesGet).mockResolvedValue({
      document: { profiles: [], standingInstructions: "" },
    });
    await act(async () => rowButton("Explorer", "Delete").click());
    await act(async () => undefined);
    await act(async () => panelButton("Delete now").click());
    await act(async () => undefined);
    await act(async () => undefined);
    // Confirmed with nothing left: the list itself holds focus.
    expect(document.activeElement?.classList.contains("agent-profile-list")).toBe(true);
  });

  it("holds the row's agents tick while that profile's dialog is open", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ enabledForAgents: true })],
      standingInstructions: "",
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    let rowTick = dom.container.querySelector<HTMLInputElement>(
      '.agent-profile-row input[type="checkbox"]',
    );
    if (!rowTick) throw new Error("row tick did not render");
    expect(rowTick.disabled).toBe(true);
    // The reason is on the screen, not a silent lock.
    expect(dom.container.textContent).toContain("The open dialog holds this setting");

    await act(async () => dialogButton("Cancel").click());
    await act(async () => undefined);
    rowTick = dom.container.querySelector<HTMLInputElement>(
      '.agent-profile-row input[type="checkbox"]',
    );
    if (!rowTick) throw new Error("row tick did not render after close");
    expect(rowTick.disabled).toBe(false);
  });
});
