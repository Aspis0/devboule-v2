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
import {
  dom,
  useAgentsPanelDom,
  makeProfile,
  storedProfile,
  renderAgentsPanel,
  typeText,
  makeProvider,
} from "./agentsPanelTestHarness";
import {
  openForm,
  nameField,
  modelControl,
  modeControl,
  createButton,
} from "./agentsPanelTestQueries";

describe("Settings agents panel — new profile form: adopting the daemon's minted ids", () => {
  useAgentsPanelDom(() => [makeProvider()]);

  /** The agents tick lives in the editor under Advanced: open, flip, save. */
  async function toggleAgentsTick(name: string) {
    const edit = dom.container.querySelector<HTMLButtonElement>(
      `.agent-profile-row button[aria-label="Edit ${name}"]`,
    );
    if (!edit) throw new Error(`Edit ${name} did not render`);
    await act(async () => edit.click());
    await act(async () => undefined);
    const advanced = Array.from(
      dom.container.querySelectorAll<HTMLButtonElement>(".edit-card button"),
    ).find((button) => button.textContent === "Advanced");
    if (!advanced) throw new Error("Advanced section did not render");
    await act(async () => advanced.click());
    await act(async () => undefined);
    const tick = dom.container.querySelector<HTMLInputElement>(
      '.edit-card input[aria-label="Available to agents"]',
    );
    if (!tick) throw new Error("agents tick did not render in Advanced");
    await act(async () => tick.click());
    await act(async () => undefined);
    const save = Array.from(
      dom.container.querySelectorAll<HTMLButtonElement>(".edit-card button"),
    ).find((button) => button.textContent === "Save");
    if (!save) throw new Error("Save did not render");
    await act(async () => save.click());
    await act(async () => undefined);
    await act(async () => undefined);
  }

  it("ticks the row the human ticked once the daemon's minted ids are adopted", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    // The store's read-back after each confirmed create carries the ids the
    // daemon minted — the set reply names only the request.
    vi.mocked(agentProfilesGet)
      .mockResolvedValueOnce({
        document: {
          profiles: [storedProfile("minted-a", { name: "Alpha" })],
          standingInstructions: "",
        },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-a", { name: "Alpha" }),
            storedProfile("minted-b", { name: "Beta" }),
          ],
          standingInstructions: "",
        },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-a", { name: "Alpha" }),
            storedProfile("minted-b", { name: "Beta", enabledForAgents: true }),
          ],
          standingInstructions: "",
        },
      });

    // Create Alpha, then Beta, letting each read-back land between them.
    await openForm();
    await typeText(nameField(), "Alpha");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    await openForm();
    await typeText(nameField(), "Beta");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(2);

    // The human enables the second profile (Beta) through its editor.
    await toggleAgentsTick("Beta");

    // The write flips exactly Beta, under the ids the daemon minted — never
    // the first empty-id row the panel used to mistake for it.
    const sent = vi.mocked(agentProfilesSet).mock.calls.at(-1)?.[0];
    expect(sent?.profiles.map((profile) => [profile.id, profile.enabledForAgents])).toEqual([
      ["minted-a", false],
      ["minted-b", true],
    ]);
  });

  it("adopts the minted ids after every confirmed write, so no empty id is ever re-sent", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    vi.mocked(agentProfilesGet)
      .mockResolvedValueOnce({
        document: {
          profiles: [storedProfile("minted-a", { name: "Alpha" })],
          standingInstructions: "",
        },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-a", { name: "Alpha" }),
            storedProfile("minted-b", { name: "Beta" }),
          ],
          standingInstructions: "",
        },
      })
      .mockResolvedValueOnce({
        document: {
          profiles: [
            storedProfile("minted-a", { name: "Alpha", enabledForAgents: true }),
            storedProfile("minted-b", { name: "Beta" }),
          ],
          standingInstructions: "",
        },
      });

    await openForm();
    await typeText(nameField(), "Alpha");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    await openForm();
    await typeText(nameField(), "Beta");
    await typeText(modelControl(), "claude-sonnet-4-5");
    await typeText(modeControl(), "default");
    await act(async () => createButton().click());
    await act(async () => undefined);
    await act(async () => undefined);

    // The second create already travelled with the first profile's minted
    // id: the read-back after write one was adopted.
    const secondCreate = vi.mocked(agentProfilesSet).mock.calls[1]?.[0];
    expect(secondCreate?.profiles.map((profile) => profile.id)).toEqual(["minted-a", ""]);

    // Enabling Alpha through its editor re-sends the whole document:
    // every id must be the daemon's — an empty id would make the store
    // mint yet another identity for a row the human already created.
    await toggleAgentsTick("Alpha");

    const tickWrite = vi.mocked(agentProfilesSet).mock.calls.at(-1)?.[0];
    expect(tickWrite?.profiles.map((profile) => profile.id)).toEqual(["minted-a", "minted-b"]);
    expect(tickWrite?.profiles.every((profile) => profile.id !== "")).toBe(true);
    expect(tickWrite?.profiles[0]?.enabledForAgents).toBe(true);
  });

  it("reverts exactly what the human was seeing when a write is refused, and adopts nothing", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "x1" })],
      standingInstructions: "",
    });
    // The one read-back armed after the load is a reply that disagrees with
    // the revert — the error path must never touch it.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          makeProfile({ id: "x1", enabledForAgents: true, note: "adopted from the store" }),
        ],
        standingInstructions: "",
      },
    });
    vi.mocked(agentProfilesSet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });

    await toggleAgentsTick("Explorer");

    // The row is exactly what the human was seeing before the edit, and
    // the refusal is named in the open dialog.
    expect(agentProfilesGet).toHaveBeenCalledTimes(1);
    expect(dom.container.querySelector('.edit-card [role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );

    // The retry is a second Save of the same draft: it re-sends the human's
    // tick over the document as it stood — never the armed reply's note.
    const save = Array.from(
      dom.container.querySelectorAll<HTMLButtonElement>(".edit-card button"),
    ).find((button) => button.textContent === "Save");
    if (!save) throw new Error("Save did not render");
    await act(async () => save.click());
    await act(async () => undefined);
    await act(async () => undefined);
    const retry = vi.mocked(agentProfilesSet).mock.calls.at(-1)?.[0];
    expect(retry?.profiles[0]?.enabledForAgents).toBe(true);
    expect(retry?.profiles[0]?.note).toBe("Reads the code and reports back.");
  });
});
