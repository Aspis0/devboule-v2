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
  renderAgentsPanel,
  typeText,
} from "./agentsPanelTestHarness";
import { rowByName, dialogButton } from "./agentsPanelTestQueries";

describe("Settings agents panel — standing instructions", () => {
  useAgentsPanelDom(() => []);

  async function openStandingEditor() {
    const edit = Array.from(
      dom.container.querySelectorAll<HTMLButtonElement>("[data-settings-row] button"),
    ).find((button) => button.textContent === "Edit");
    if (!edit) throw new Error("standing instructions Edit row did not render");
    await act(async () => edit.click());
    await act(async () => undefined);
    const box = dom.container.querySelector<HTMLTextAreaElement>(
      '[aria-label="Standing instructions for every agent"]',
    );
    if (!box) throw new Error("the standing instructions editor did not render");
    return box;
  }

  it("edits standing instructions behind an Edit row, with no inline box", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "Rules to reuse." });
    // The page is rows: no textarea until the editor opens.
    expect(
      dom.container.querySelector('[aria-label="Standing instructions for every agent"]'),
    ).toBeNull();
    const box = await openStandingEditor();
    expect(box.value).toBe("Rules to reuse.");
    // No explanatory hint in the editor — one short label, the counter only
    // near the cap.
    expect(box.closest(".edit-card")?.querySelectorAll(".device-field-hint").length).toBe(0);
  });

  it("saves standing instructions into the document and leaves the profiles alone", async () => {
    const explorer = makeProfile();
    await renderAgentsPanel({ profiles: [explorer], standingInstructions: "" });
    // The store's read-back after the confirmed save: the text is stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [explorer],
        standingInstructions: "Report your result in your final message.",
      },
    });

    const field = await openStandingEditor();
    await typeText(field, "Report your result in your final message.");
    await act(async () => dialogButton("Save standing instructions").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [explorer],
      standingInstructions: "Report your result in your final message.",
    });
    // Confirmed: the editor closes on the write that carried its text.
    expect(
      dom.container.querySelector('[aria-label="Standing instructions for every agent"]'),
    ).toBeNull();
  });

  it("refuses standing instructions over 8 KiB with the size named and truncates nothing", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    const field = await openStandingEditor();
    // 4200 two-byte characters: 8400 UTF-8 bytes, 208 over the cap.
    const flood = "é".repeat(4200);
    await typeText(field, flood);

    await act(async () => dialogButton("Save standing instructions").click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = dom.container.querySelector(".edit-card [role='alert']");
    expect(alert?.textContent).toContain("8400 bytes");
    expect(alert?.textContent).toContain("8192");
    expect(field.value).toBe(flood);
  });

  it("keeps the standing draft when an unrelated write confirms", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile(), makeProfile({ id: "profile-2", name: "Coder" })],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed move: no draft in it.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [makeProfile({ id: "profile-2", name: "Coder" }), makeProfile()],
        standingInstructions: "",
      },
    });

    const field = await openStandingEditor();
    await typeText(field, "Always report your plan first.");

    // An unrelated write — a reorder. It sends the document as the store
    // holds it (the draft is deliberately not smuggled into it), and when
    // it confirms the typed text must still be in the editor: the move did
    // not carry the text, so releasing the draft would destroy words no
    // write ever took.
    const up = rowByName("Coder").querySelector<HTMLButtonElement>(
      'button[aria-label="Move Coder up"]',
    );
    if (!up) throw new Error("move-up button did not render");
    await act(async () => up.click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [makeProfile({ id: "profile-2", name: "Coder" }), makeProfile()],
      standingInstructions: "",
    });
    const fieldAfter = dom.container.querySelector<HTMLTextAreaElement>(
      '[aria-label="Standing instructions for every agent"]',
    );
    expect(fieldAfter?.value).toBe("Always report your plan first.");
  });

  it("keeps keystrokes typed while the standing save itself was in flight", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed save: it holds what was sent.
    vi.mocked(agentProfilesGet).mockResolvedValue({
      document: { profiles: [makeProfile()], standingInstructions: "Always report" },
    });

    const field = await openStandingEditor();
    await typeText(field, "Always report");
    let resolveSet: (() => void) | undefined;
    vi.mocked(agentProfilesSet).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveSet = resolve;
        }),
    );
    await act(async () => dialogButton("Save standing instructions").click());
    await act(async () => undefined);

    // The write is in flight carrying "Always report"; the human keeps
    // typing. The confirmation may release a draft that is still what was
    // sent — this tail is newer than the store and must survive it, so the
    // editor closes on the confirmed text and reopening shows the tail.
    await typeText(field, "Always report your plan first.");
    await act(async () => {
      resolveSet?.();
    });
    await act(async () => undefined);
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [makeProfile()],
      standingInstructions: "Always report",
    });
    const reopened = await openStandingEditor();
    expect(reopened.value).toBe("Always report your plan first.");
  });
});
