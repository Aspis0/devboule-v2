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

function providerSelect(): HTMLSelectElement {
  const select = dom.container.querySelector<HTMLSelectElement>('select[aria-label="Provider"]');
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

const DOCUMENT = {
  profiles: [
    storedProfile("p-gone", { name: "Gone", provider: "does-not-exist", model: "old-model" }),
    storedProfile("p-good", { name: "Good", provider: "claude" }),
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

  it("locks every field but the provider until another installed provider is picked", async () => {
    await renderAgentsPanel(DOCUMENT);
    await act(async () => undefined);
    await openRowEditor("Gone");
    const options = Array.from(providerSelect().options).map((option) => option.textContent);
    expect(providerSelect().value).toBe("does-not-exist");
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
