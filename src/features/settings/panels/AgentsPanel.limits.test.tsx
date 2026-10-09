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
import { rowButton, panelButton } from "./agentsPanelTestQueries";

describe("Settings agents panel — spawn prompt, note and name limits", () => {
  useAgentsPanelDom(() => []);

  it("accepts a spawn prompt whose trimmed bytes fit the cap", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const spawnField = dom.container.querySelector<HTMLTextAreaElement>(
      '.agent-inline-editor textarea[aria-label="Profile instructions"]',
    );
    if (!spawnField) throw new Error("spawn prompt field did not render");
    // One U+0085 over the cap raw, exactly the cap after the daemon's trim:
    // Rust's `str::trim` drops U+0085 and ECMAScript's `trim()` keeps it, so
    // the preflight must trim the way Rust trims — a JS-trimmed count is
    // 8194 bytes and would refuse a prompt the store accepts.
    const prompt = `\u{0085}${"a".repeat(8192)}`;
    await typeText(spawnField, prompt);

    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    expect(sent.profiles[0]?.spawnPrompt).toBe("a".repeat(8192));
  });

  it("refuses a spawn prompt over 8 KiB with the size named and truncates nothing", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const spawnField = dom.container.querySelector<HTMLTextAreaElement>(
      '.agent-inline-editor textarea[aria-label="Profile instructions"]',
    );
    if (!spawnField) throw new Error("spawn prompt field did not render");
    // 4097 two-byte characters: 8194 UTF-8 bytes, 2 over the cap. The byte
    // count is what the daemon enforces, so a char-counting UI would pass it.
    const flood = "é".repeat(4097);
    await typeText(spawnField, flood);

    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = dom.container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("8194");
    expect(alert?.textContent).toContain("8192");
    // The refusal changed nothing: the field still holds every byte.
    expect(spawnField.value).toBe(flood);
  });

  it("saves a cleared spawn prompt as the field's absence, never as an empty string", async () => {
    const carrying = makeProfile({ spawnPrompt: "Check the diff before you report." });
    await renderAgentsPanel({ profiles: [carrying], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const spawnField = dom.container.querySelector<HTMLTextAreaElement>(
      '.agent-inline-editor textarea[aria-label="Profile instructions"]',
    );
    if (!spawnField) throw new Error("spawn prompt field did not render");
    // Whitespace only: the daemon trims the field, so this is none, and the
    // wire shape of none is the key's absence.
    await typeText(spawnField, "   ");

    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const sent = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    expect("spawnPrompt" in sent.profiles[0]).toBe(false);
  });

  it("keeps the instructions field to one short label, no explanation", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const editor = dom.container.querySelector(".agent-inline-editor");
    if (!editor) throw new Error("editor did not render");
    const label = editor
      .querySelector('textarea[aria-label="Profile instructions"]')
      ?.closest("label");
    if (!label) throw new Error("instructions label did not render");
    // One short label, no hint paragraph, no counter before the cap.
    expect(label.querySelectorAll(".device-field-hint").length).toBe(0);
    expect(label.textContent).toContain("Instructions (optional)");
  });

  it("refuses a note over 2 KiB with the size named and truncates nothing", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const noteField = dom.container.querySelector<HTMLTextAreaElement>(
      ".agent-inline-editor textarea",
    );
    if (!noteField) throw new Error("note field did not render");
    // 1100 two-byte characters: 2200 UTF-8 bytes, 152 over the cap. The byte
    // count is what the daemon enforces, so a char-counting UI would pass it.
    const flood = "é".repeat(1100);
    await typeText(noteField, flood);

    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = dom.container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("2200 bytes");
    expect(alert?.textContent).toContain("2048");
    // The refusal changed nothing: the field still holds every byte.
    expect(noteField.value).toBe(flood);
  });

  it("accepts a name at the daemon's own count: 40 astral-plane characters are 40 characters", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const nameField = dom.container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    if (!nameField) throw new Error("name field did not render");
    // 40 emoji are 40 Unicode scalar values — what the daemon counts — but
    // 80 UTF-16 code units. A length-counting panel would refuse a legal
    // name; this one must send it.
    const name = "🦄".repeat(40);
    await typeText(nameField, name);

    // The store's read-back after the confirmed save: the name is stored.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [{ ...makeProfile(), name }], standingInstructions: "" },
    });

    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(dom.container.querySelector('[role="alert"]')).toBeNull();
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [{ ...makeProfile(), name, modelProvider: null }],
      standingInstructions: "",
    });
  });

  it("refuses a name past the daemon's count with the daemon's number", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);

    const nameField = dom.container.querySelector<HTMLInputElement>(".agent-inline-editor input");
    if (!nameField) throw new Error("name field did not render");
    // 61 emoji are 61 scalar values — 61 for the daemon too — but 122 UTF-16
    // code units. The refusal must name 61, the daemon's number, never 122.
    const name = "🦄".repeat(61);
    await typeText(nameField, name);

    await act(async () => panelButton("Save").click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = dom.container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("61 characters");
    expect(alert?.textContent).toContain("60-character cap");
    // The refusal truncates nothing and leaves the editor open.
    expect(nameField.value).toBe(name);
  });
});
