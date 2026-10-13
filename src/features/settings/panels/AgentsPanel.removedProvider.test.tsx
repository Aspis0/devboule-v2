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

import {
  dom,
  makeProvider,
  renderAgentsPanel,
  storedProfile,
  typeText,
  useAgentsPanelDom,
} from "./agentsPanelTestHarness";
import { openRowEditor, panelButton, rowByName } from "./agentsPanelTestQueries";
import { selectOptions, valueOf } from "../../../components/appSelectTesting";

function providerSelect(): HTMLButtonElement {
  const select = dom.container.querySelector<HTMLButtonElement>(
    'button[aria-haspopup=\"listbox\"][aria-label=\"Provider\"]',
  );
  if (!select) throw new Error("provider picker did not render");
  return select;
}

function fieldNamed(label: string): HTMLInputElement | HTMLTextAreaElement {
  const field = dom.container.querySelector<HTMLInputElement | HTMLTextAreaElement>(
    `[aria-label="${label}"]`,
  );
  if (!field) throw new Error(`${label} did not render`);
  return field;
}

/** A built-in Design and a personal profile may share one name, so this suite
 * keys the row it means on the id the daemon stored. */
function rowById(id: string): HTMLElement {
  const row = dom.container.querySelector<HTMLElement>(`[data-profile-id="${id}"]`);
  if (!row) throw new Error(`profile row ${id} did not render`);
  return row;
}

const DOCUMENT = {
  profiles: [
    storedProfile("p-gone", { name: "Gone", provider: "does-not-exist", model: "old-model" }),
    storedProfile("p-good", { name: "Good", provider: "claude" }),
  ],
  standingInstructions: "",
};

const DESIGN_ROWS = {
  profiles: [
    // The built-in, turned off: it stores no provider because it runs on
    // whichever provider created it.
    storedProfile("design-builtin", {
      name: "Design",
      provider: "",
      model: "",
      modeId: "",
      enabledForAgents: false,
    }),
    storedProfile("p-design-gone", {
      name: "Design",
      provider: "does-not-exist",
      model: "old-model",
    }),
  ],
  standingInstructions: "",
};

describe("Settings agents panel — a profile whose provider is not installed", () => {
  useAgentsPanelDom(() => [makeProvider({ id: "claude" })]);

  it("labels its row as not installed and leaves an installed row unlabelled", async () => {
    await renderAgentsPanel(DOCUMENT);
    await act(async () => undefined);
    expect(rowByName("Gone").textContent).toContain("not installed");
    expect(rowByName("Good").textContent).not.toContain("not installed");
  });

  it("names the creating agent's provider on the built-in row instead of calling it uninstalled", async () => {
    await renderAgentsPanel(DESIGN_ROWS);
    await act(async () => undefined);

    const builtin = rowById("design-builtin");
    expect(builtin.querySelector(".profile-meta")?.textContent).toBe(
      "Uses the creating agent's provider",
    );

    // The name alone buys nothing: a personal Design keeps its stored meta and
    // its missing provider called out.
    const personal = rowById("p-design-gone");
    expect(personal.querySelector(".profile-meta")?.textContent).toBe(
      "does-not-exist · old-model · not installed",
    );
  });

  it("locks every field but the provider until another installed provider is picked", async () => {
    await renderAgentsPanel(DOCUMENT);
    await act(async () => undefined);
    await openRowEditor("Gone");
    const options = (await selectOptions(providerSelect())).map((option) => option.label);
    expect(valueOf(providerSelect())).toBe("does-not-exist");
    expect(options).toContain("does-not-exist (not installed)");
    expect(fieldNamed("Profile name").disabled).toBe(true);
    expect(fieldNamed("Profile note").disabled).toBe(true);
    expect(fieldNamed("Profile instructions").disabled).toBe(true);
    expect(panelButton("Save").disabled).toBe(true);

    await typeText(providerSelect(), "claude");
    await act(async () => undefined);
    expect(fieldNamed("Profile name").disabled).toBe(false);
    expect(panelButton("Save").disabled).toBe(false);
  });
});
