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

import { providersList } from "../../../lib/tauri";
import {
  dom,
  useAgentsPanelDom,
  makeProfile,
  makeProvider,
  storedProfile,
  renderAgentsPanel,
} from "./agentsPanelTestHarness";
import {
  openForm,
  form,
  providerField,
  selectValues,
  newProfileButton,
} from "./agentsPanelTestQueries";

describe("Settings agents panel — new profile form: catalog, caps and stored rows", () => {
  useAgentsPanelDom(() => [makeProvider()]);

  it("shows the peer restriction in the stored profile's editor", async () => {
    await renderAgentsPanel({
      profiles: [
        storedProfile("p-1", {
          name: "Hermit",
          toolOverlay: ["devboule_send_message", "devboule_create_agent"],
        }),
        storedProfile("p-2", { name: "Social" }),
      ],
      standingInstructions: "",
    });

    // The rows stay quiet; the restriction reads in the editor under
    // Advanced, ticked on for the stored pair.
    expect(dom.container.textContent).not.toContain("cannot message peers");
    const edit = dom.container.querySelector<HTMLButtonElement>(
      '.agent-profile-row button[aria-label="Edit Hermit"]',
    );
    if (!edit) throw new Error("Edit Hermit did not render");
    await act(async () => edit.click());
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
    if (!peersTick) throw new Error("peer tick did not render");
    expect(peersTick.checked).toBe(true);
  });

  it("names the denial in the editor when the overlay is not the exact peer pair", async () => {
    // A single-tool denial is valid daemon-side; the editor must name it
    // instead of showing nothing.
    await renderAgentsPanel({
      profiles: [
        storedProfile("p-1", {
          name: "NoGrandchildren",
          toolOverlay: ["devboule_create_agent", "devboule_list_profiles"],
        }),
      ],
      standingInstructions: "",
    });

    const text = dom.container.textContent ?? "";
    expect(text).not.toContain("cannot use:");
    const edit = dom.container.querySelector<HTMLButtonElement>(
      '.agent-profile-row button[aria-label="Edit NoGrandchildren"]',
    );
    if (!edit) throw new Error("Edit NoGrandchildren did not render");
    await act(async () => edit.click());
    await act(async () => undefined);
    const advanced = Array.from(
      dom.container.querySelectorAll<HTMLButtonElement>(".edit-card button"),
    ).find((button) => button.textContent === "Advanced");
    if (!advanced) throw new Error("Advanced section did not render");
    await act(async () => advanced.click());
    await act(async () => undefined);
    const editorText = dom.container.querySelector(".edit-card")?.textContent ?? "";
    expect(editorText).toContain("cannot use: devboule_list_profiles");
    expect(editorText).not.toContain("cannot message peers");
  });

  it("offers only installed providers in the picker", async () => {
    vi.mocked(providersList).mockImplementation(async () => ({
      providers: [
        makeProvider({ id: "claude" }),
        makeProvider({ id: "codex", installed: false, protocol: null }),
      ],
      unreadableDirs: 0,
    }));
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();

    expect(selectValues(providerField())).toEqual(["claude"]);
  });

  it("does not claim no agent CLI is installed when the catalog read failed", async () => {
    // Every providers_list caller is refused: the catalog was never read.
    vi.mocked(providersList).mockRejectedValue({ code: "io", message: "the scan failed" });
    await renderAgentsPanel({ profiles: [], standingInstructions: "" });
    await openForm();

    // The failed read names itself; the empty-catalog claim is not made.
    expect(form().textContent).toContain(
      "could not be read: A system or file operation failed on this machine.",
    );
    expect(form().textContent).not.toContain("No agent CLI is installed");
    // The picker does not pretend the (unread) catalog was read either: its
    // one option names the failed read, not an empty result. (Placeholder
    // options carry value="", so the option text is what is asserted.)
    const options = Array.from(providerField().options).map((option) => option.textContent);
    expect(options).toEqual(["The catalog could not be read"]);
  });

  it("mirrors the store's profile cap and does not offer the form at it", async () => {
    const full = Array.from({ length: 64 }, (_, index) =>
      makeProfile({ id: `p-${index}`, name: `Profile ${index}` }),
    );
    await renderAgentsPanel({ profiles: full, standingInstructions: "" });

    // The cap is named before the human fills anything in...
    expect(dom.container.textContent).toContain("the maximum of 64 profiles");
    // ...and the form cannot be opened: a 65th creation is refused by the
    // store, so the section action stays disabled.
    const open = newProfileButton();
    expect(open.disabled).toBe(true);
    await act(async () => open?.click());
    await act(async () => undefined);
    expect(dom.container.querySelector(".agent-profile-create")).toBeNull();
  });
});
