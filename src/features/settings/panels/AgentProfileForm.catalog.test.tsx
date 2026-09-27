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
import { openForm, form, providerField, selectValues } from "./agentsPanelTestQueries";

describe("Settings agents panel — new profile form: catalog, caps and stored rows", () => {
  useAgentsPanelDom(() => [makeProvider()]);

  it("shows the peer restriction on a stored profile row", async () => {
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

    const hermit = dom.container.textContent ?? "";
    expect(hermit).toContain("cannot message peers or create further agents");
  });

  it("names the denial on a row whose overlay is not the exact peer pair", async () => {
    // A single-tool denial is valid daemon-side; the row must render it
    // instead of showing nothing.
    await renderAgentsPanel({
      profiles: [
        storedProfile("p-1", {
          name: "NoGrandchildren",
          toolOverlay: ["devboule_create_agent"],
        }),
      ],
      standingInstructions: "",
    });

    const text = dom.container.textContent ?? "";
    expect(text).toContain("cannot use: devboule_create_agent");
    expect(text).not.toContain("cannot message peers");
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
    // store, so the panel does not offer the work.
    const open = Array.from(
      dom.container.querySelectorAll<HTMLButtonElement>(".agent-profile-create-row button"),
    ).find((candidate) => candidate.textContent === "New profile");
    expect(open?.disabled).toBe(true);
    await act(async () => open?.click());
    await act(async () => undefined);
    expect(dom.container.querySelector(".agent-profile-create")).toBeNull();
  });
});
