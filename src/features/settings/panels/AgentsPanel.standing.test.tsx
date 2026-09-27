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
  describedText,
  typeText,
} from "./agentsPanelTestHarness";
import { tickBox, panelButton } from "./agentsPanelTestQueries";

describe("Settings agents panel — standing instructions", () => {
  useAgentsPanelDom(() => []);

  it("ties one hint to the standing instructions box, keep-it-short sentence included", async () => {
    await renderAgentsPanel({ profiles: [], standingInstructions: "Rules to reuse." });

    const box = dom.container.querySelector<HTMLTextAreaElement>(
      '[aria-label="Standing instructions for every agent"]',
    );
    if (!box) throw new Error("the standing instructions box did not render");
    // One field-hint in the box — the section's `device-copy` description is
    // not a hint, so nothing was merged there — tied through described-by.
    expect(box.closest(".agent-standing")?.querySelectorAll(".device-field-hint").length).toBe(1);
    expect(describedText(box)).toBe("Keep it short: every agent also receives its own task.");
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

    const field = dom.container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!field) throw new Error("standing instructions field did not render");
    await typeText(field, "Report your result in your final message.");

    await act(async () => panelButton("Save standing instructions").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [explorer],
      standingInstructions: "Report your result in your final message.",
    });
  });

  it("refuses standing instructions over 8 KiB with the size named and truncates nothing", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    const field = dom.container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!field) throw new Error("standing instructions field did not render");
    // 4200 two-byte characters: 8400 UTF-8 bytes, 208 over the cap.
    const flood = "é".repeat(4200);
    await typeText(field, flood);

    await act(async () => panelButton("Save standing instructions").click());
    await act(async () => undefined);

    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = dom.container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("8400 bytes");
    expect(alert?.textContent).toContain("8192");
    expect(field.value).toBe(flood);
  });

  it("keeps the standing draft on screen when an unrelated write confirms", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed write: no draft in it.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...makeProfile(), enabledForAgents: true }],
        standingInstructions: "",
      },
    });

    const field = dom.container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!field) throw new Error("standing instructions field did not render");
    await typeText(field, "Always report your plan first.");

    // An unrelated write — the tick. It sends the document as the store
    // holds it (the draft is deliberately not smuggled into it), and when
    // it confirms the typed text must still be in the box: the tick did not
    // carry the text, so releasing the draft would destroy words no write
    // ever took. An earlier version of this test pinned the opposite — the
    // release — which is the silent loss the cumulative audit's finding 1.
    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [{ ...makeProfile(), enabledForAgents: true }],
      standingInstructions: "",
    });
    const fieldAfter = dom.container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    expect(fieldAfter?.value).toBe("Always report your plan first.");
  });

  it("keeps keystrokes typed while the standing save itself was in flight", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    // The store's read-back after the confirmed save: it holds what was sent.
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [makeProfile()], standingInstructions: "Always report" },
    });

    const field = dom.container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    if (!field) throw new Error("standing instructions field did not render");
    await typeText(field, "Always report");
    let resolveSet: (() => void) | undefined;
    vi.mocked(agentProfilesSet).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveSet = resolve;
        }),
    );
    await act(async () => panelButton("Save standing instructions").click());
    await act(async () => undefined);

    // The write is in flight carrying "Always report"; the human keeps
    // typing (the box is deliberately editable mid-write). The confirmation
    // may release a draft that is still what was sent — this tail is newer
    // than the store and must survive it.
    await typeText(field, "Always report your plan first.");
    await act(async () => {
      resolveSet?.();
    });
    await act(async () => undefined);

    expect(agentProfilesSet).toHaveBeenCalledWith({
      profiles: [makeProfile()],
      standingInstructions: "Always report",
    });
    const fieldAfter = dom.container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
    expect(fieldAfter?.value).toBe("Always report your plan first.");
  });
});
