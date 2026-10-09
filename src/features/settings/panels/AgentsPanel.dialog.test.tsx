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
import {
  rowByName,
  rowButton,
  panelButton,
  dialogButton,
  newProfileButton,
} from "./agentsPanelTestQueries";

describe("Settings agents panel — the profile dialog", () => {
  useAgentsPanelDom(() => []);

  it("opens one dialog for creating and one for editing, with the full spawn text", async () => {
    const prompt =
      "First line of the standing orders. Second line with the details. Third line nobody clamps.";
    await renderAgentsPanel({
      profiles: [makeProfile({ spawnPrompt: prompt })],
      standingInstructions: "",
    });

    await act(async () => newProfileButton().click());
    await act(async () => undefined);
    expect(dom.container.querySelector(".edit-scrim")).not.toBeNull();
    expect(dom.container.querySelector(".edit-title")?.textContent).toBe("New profile");
    await act(async () => dialogButton("Cancel").click());
    await act(async () => undefined);
    expect(dom.container.querySelector(".edit-scrim")).toBeNull();

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    expect(dom.container.querySelector(".edit-title")?.textContent).toBe("Edit profile — Explorer");
    // The row shows name, meta and note; the dialog carries the whole text.
    const spawnField = dom.container.querySelector<HTMLTextAreaElement>(
      '.edit-card textarea[aria-label="Profile instructions"]',
    );
    if (!spawnField) throw new Error("dialog spawn prompt field did not render");
    expect(spawnField.value).toBe(prompt);
  });

  it("returns focus to the row's pencil when the dialog closes", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    const pencil = rowByName("Explorer").querySelector<HTMLButtonElement>(
      'button[aria-label="Edit Explorer"]',
    );
    if (!pencil) throw new Error("row pencil did not render");
    pencil.focus();
    await act(async () => pencil.click());
    await act(async () => undefined);
    expect(dom.container.querySelector(".edit-scrim")).not.toBeNull();

    await act(async () => dialogButton("Cancel").click());
    await act(async () => undefined);
    expect(dom.container.querySelector(".edit-scrim")).toBeNull();
    expect(document.activeElement).toBe(pencil);
  });

  it("holds the panel dialog's Escape behind the discard check when dirty", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const nameField = dom.container.querySelector<HTMLInputElement>(
      '.edit-card input[aria-label="Profile name"]',
    );
    if (!nameField) throw new Error("dialog name field did not render");
    await typeText(nameField, "Scout");

    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(dom.container.querySelector(".edit-scrim")).not.toBeNull();
    expect(dom.container.textContent).toContain("Discard unsaved changes?");

    await act(async () => dialogButton("Discard").click());
    await act(async () => undefined);
    expect(dom.container.querySelector(".edit-scrim")).toBeNull();
  });

  it("shows a refused save inside the dialog card, next to Save", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const nameField = dom.container.querySelector<HTMLInputElement>(
      '.edit-card input[aria-label="Profile name"]',
    );
    if (!nameField) throw new Error("dialog name field did not render");
    await typeText(nameField, "🦄".repeat(61));
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    // The dialog stays open, and the sentence is inside the card — not
    // painted behind the scrim at the top of the pane.
    expect(dom.container.querySelector(".edit-scrim")).not.toBeNull();
    const cardAlert = dom.container.querySelector('.edit-card [role="alert"]');
    expect(cardAlert?.textContent).toContain("61 characters");
    expect(dom.container.querySelector('.agent-profiles > [role="alert"]')).toBeNull();
  });

  it("hints what the Icon field takes", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const advanced = Array.from(
      dom.container.querySelectorAll<HTMLButtonElement>(".edit-card button"),
    ).find((button) => button.textContent === "Advanced");
    if (!advanced) throw new Error("Advanced section did not render");
    await act(async () => advanced.click());
    await act(async () => undefined);
    const hint = dom.container.querySelector('.edit-card [id$="-icon-hint"]');
    if (!hint) throw new Error("icon hint did not render");
    expect(hint.textContent).toContain("first letter");
  });

  it("lands mid-save close focus on the list, and re-owns the pencil on settle", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [makeProfile({ name: "Scout" })], standingInstructions: "" },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const nameField = dom.container.querySelector<HTMLInputElement>(
      '.edit-card input[aria-label="Profile name"]',
    );
    if (!nameField) throw new Error("dialog name field did not render");
    await typeText(nameField, "Scout");

    let resolveSet: (() => void) | undefined;
    vi.mocked(agentProfilesSet).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveSet = resolve;
        }),
    );
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);
    // The opener is disabled mid-save: closing must land on the list,
    // not drop focus to the body.
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    await act(async () => dialogButton("Close dialog").click());
    await act(async () => undefined);
    expect(dom.container.querySelector(".edit-scrim")).toBeNull();
    expect(document.activeElement?.classList.contains("agent-profile-list")).toBe(true);

    // The write settles into a closed dialog: the pencil is re-owned.
    await act(async () => {
      resolveSet?.();
    });
    await act(async () => undefined);
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Edit Scout");
  });

  it("holds the dialog's exits while its save is in flight", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [makeProfile({ name: "Scout" })], standingInstructions: "" },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const nameField = dom.container.querySelector<HTMLInputElement>(
      '.edit-card input[aria-label="Profile name"]',
    );
    if (!nameField) throw new Error("dialog name field did not render");
    await typeText(nameField, "Scout");

    let resolveSet: (() => void) | undefined;
    vi.mocked(agentProfilesSet).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveSet = resolve;
        }),
    );
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    // In flight: Escape arms the honest exit, it does not close; the card
    // stays, and abandoning the view is a second, named click.
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(dom.container.querySelector(".edit-scrim")).not.toBeNull();
    expect(dom.container.textContent).toContain("A save is still running.");
    expect(dom.container.textContent).not.toContain("Discard unsaved changes?");
    await act(async () => dialogButton("Close dialog").click());
    await act(async () => undefined);
    expect(dom.container.querySelector(".edit-scrim")).toBeNull();

    await act(async () => {
      resolveSet?.();
    });
    await act(async () => undefined);
    // The write still landed after the view was abandoned.
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
  });

  it("drops no stale refusal into the next dialog", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile(), makeProfile({ id: "profile-2", name: "Coder" })],
      standingInstructions: "",
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const nameField = dom.container.querySelector<HTMLInputElement>(
      '.edit-card input[aria-label="Profile name"]',
    );
    if (!nameField) throw new Error("dialog name field did not render");
    await typeText(nameField, "");
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);
    expect(dom.container.querySelector('.edit-card [role="alert"]')).not.toBeNull();
    // Abandon the dirty dialog through the check, then open another row:
    // the first row's sentence must not greet the second.
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    await act(async () => dialogButton("Discard").click());
    await act(async () => undefined);
    await act(async () => rowButton("Coder", "Edit").click());
    await act(async () => undefined);
    expect(dom.container.querySelector('.edit-card [role="alert"]')).toBeNull();
  });

  it("keeps the editor's draft on screen under its error when a rename is refused", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "x1" })],
      standingInstructions: "",
    });
    // The store's read-back after the (only) confirmed save, at the retry.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [
          makeProfile({ id: "x1", name: "Scout", note: "Maps the work before anyone builds." }),
        ],
        standingInstructions: "",
      },
    });
    vi.mocked(agentProfilesSet).mockRejectedValueOnce({
      code: "io",
      message: "A system or file operation failed on this machine.",
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editorNameField = dom.container.querySelector<HTMLInputElement>(
      ".agent-inline-editor input",
    );
    const editorNoteField = dom.container.querySelector<HTMLTextAreaElement>(
      ".agent-inline-editor textarea",
    );
    if (!editorNameField || !editorNoteField) throw new Error("editor fields did not render");
    await typeText(editorNameField, "Scout");
    await typeText(editorNoteField, "Maps the work before anyone builds.");

    await act(async () => panelButton("Save").click());
    await act(async () => undefined);
    await act(async () => undefined);

    // The refusal is named, the editor still stands, and the draft is in
    // its fields — the create form's rule, held here too.
    expect(dom.container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    const editor = dom.container.querySelector(".agent-inline-editor");
    expect(editor).not.toBeNull();
    expect(editor?.querySelector<HTMLInputElement>("input")?.value).toBe("Scout");
    expect(editor?.querySelector<HTMLTextAreaElement>("textarea")?.value).toBe(
      "Maps the work before anyone builds.",
    );
    // The row under it is exactly what the human was seeing before.
    expect(rowByName("Explorer").querySelector(".profile-name")?.textContent).toBe("Explorer");

    // The retry sends the same draft, and confirmation — never submission —
    // closes the editor.
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(2);
    const sent = vi.mocked(agentProfilesSet).mock.calls.at(-1)?.[0];
    expect(sent?.profiles[0]?.name).toBe("Scout");
    expect(sent?.profiles[0]?.note).toBe("Maps the work before anyone builds.");
    expect(dom.container.querySelector(".agent-inline-editor")).toBeNull();
  });

  it("keeps the editor's draft when a refused delete removes and restores its row", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "x1" }), makeProfile({ id: "x2", name: "Coder" })],
      standingInstructions: "",
    });
    // The delete's fate is held outside, so each stage of the sequence is
    // observable deterministically: the optimistic removal, then the
    // refusal's revert.
    let rejectSet!: (cause: unknown) => void;
    vi.mocked(agentProfilesSet).mockImplementationOnce(
      () =>
        new Promise<void>((_resolve, reject) => {
          rejectSet = reject;
        }),
    );

    // Open the editor on the row that will be deleted, and type into it.
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const nameField = dom.container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    const noteField = dom.container.querySelector<HTMLTextAreaElement>(
      ".agent-inline-editor textarea",
    );
    if (!nameField || !noteField) throw new Error("editor fields did not render");
    await typeText(nameField, "Scout");
    await typeText(noteField, "Maps the work before anyone builds.");

    // Delete that same row: the optimistic removal unmounts the editor —
    // the row, and the editor rendered inside it, are gone from the screen.
    await act(async () => rowButton("Explorer", "Delete").click());
    await act(async () => panelButton("Delete now").click());
    await act(async () => undefined);
    expect(dom.container.querySelector(".agent-inline-editor")).toBeNull();

    // The write is refused; the revert brings the row back and the editor
    // remounts. It must come back with the draft in its fields, under the
    // error — the rule its own write obeys, held for a write that removed
    // the row. A draft kept inside the editor's own state would remount
    // empty here; it lives one level up for exactly this.
    await act(async () => {
      rejectSet({ code: "io", message: "profile file unwritable" });
    });
    await act(async () => undefined);
    expect(dom.container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    const editor = dom.container.querySelector(".agent-inline-editor");
    expect(editor).not.toBeNull();
    expect(editor?.querySelector<HTMLInputElement>("input")?.value).toBe("Scout");
    expect(editor?.querySelector<HTMLTextAreaElement>("textarea")?.value).toBe(
      "Maps the work before anyone builds.",
    );
    expect(rowByName("Explorer").querySelector(".profile-name")?.textContent).toBe("Explorer");
  });

  it("keeps the editor's draft across a confirmed write from another row", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "x1" }), makeProfile({ id: "x2", name: "Coder" })],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed move.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [makeProfile({ id: "x2", name: "Coder" }), makeProfile({ id: "x1" })],
        standingInstructions: "",
      },
    });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const nameField = dom.container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    if (!nameField) throw new Error("editor name field did not render");
    await typeText(nameField, "Scout");

    // Another row's write, confirmed with its read-back: the editor stays
    // open and the draft stays in it — no write that did not carry the
    // draft may release it.
    const up = rowByName("Coder").querySelector<HTMLButtonElement>(
      'button[aria-label="Move Coder up"]',
    );
    if (!up) throw new Error("move-up button did not render");
    await act(async () => up.click());
    await act(async () => undefined);

    const editor = dom.container.querySelector(".agent-inline-editor");
    expect(editor).not.toBeNull();
    expect(editor?.querySelector<HTMLInputElement>("input")?.value).toBe("Scout");
  });
});
