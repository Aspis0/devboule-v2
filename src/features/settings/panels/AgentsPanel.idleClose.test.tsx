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
  describedText,
  typeText,
} from "./agentsPanelTestHarness";
import { rowButton, panelButton } from "./agentsPanelTestQueries";

describe("Settings agents panel — the idle-close timer", () => {
  useAgentsPanelDom(() => []);

  it("round-trips the idle-close timer: the default 30, a custom value, and off", async () => {
    const explorer = makeProfile();
    await renderAgentsPanel({ profiles: [explorer], standingInstructions: "" });
    const idleField = (): HTMLInputElement => {
      const field = dom.container.querySelector<HTMLInputElement>(
        '.agent-inline-editor input[aria-label="Close idle children after minutes"]',
      );
      if (!field) throw new Error("the idle-close minutes field did not render");
      return field;
    };
    const offTick = (): HTMLInputElement => {
      const tick = dom.container.querySelector<HTMLInputElement>(
        '.agent-inline-editor input[aria-label="Never close idle children"]',
      );
      if (!tick) throw new Error("the idle-close off toggle did not render");
      return tick;
    };

    // 1. The default: a profile that says nothing opens showing 30, and a
    // save that never touched the field keeps the key out — absent is what
    // means 30 to the daemon, so an untouched field leaves the row alone.
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    expect(idleField().value).toBe("30");
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [explorer], standingInstructions: "" },
    });
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    const first = vi.mocked(agentProfilesSet).mock.calls[0]?.[0] as AgentProfilesDocument;
    expect("idleCloseMinutes" in first.profiles[0]).toBe(false);

    // 2. A custom value: typed, stored under the key, and shown again on the
    // next open — the round trip the field exists for.
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    await typeText(idleField(), "45");
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...explorer, idleCloseMinutes: 45 }],
        standingInstructions: "",
      },
    });
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenLastCalledWith({
      profiles: [{ ...explorer, idleCloseMinutes: 45 }],
      standingInstructions: "",
    });
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    expect(idleField().value).toBe("45");
    expect(offTick().checked).toBe(false);

    // 3. Off: the tick saves the daemon's `Some(0)`, and reopening it shows
    // the tick still on, the field at the default and out of the way.
    await act(async () => offTick().click());
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...explorer, idleCloseMinutes: 0 }],
        standingInstructions: "",
      },
    });
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenLastCalledWith({
      profiles: [{ ...explorer, idleCloseMinutes: 0 }],
      standingInstructions: "",
    });
    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    expect(offTick().checked).toBe(true);
    expect(idleField().disabled).toBe(true);
    expect(idleField().value).toBe("30");
  });

  it("never lets a typed 0 in the minutes field mean never", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const idleField = dom.container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Close idle children after minutes"]',
    );
    const offTick = dom.container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Never close idle children"]',
    );
    if (!idleField || !offTick) throw new Error("the idle-close controls did not render");

    await typeText(idleField, "0");
    // The field shows what will be saved: a whole minute. "Never" is the
    // tick's meaning alone, so a typed 0 must not reach the daemon as
    // `Some(0)` — the opposite of what the human asked for.
    expect(idleField.value).toBe("1");
    expect(offTick.checked).toBe(false);

    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: {
        profiles: [{ ...makeProfile(), idleCloseMinutes: 1 }],
        standingInstructions: "",
      },
    });
    await act(async () => panelButton("Save").click());
    await act(async () => undefined);
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesSet).toHaveBeenLastCalledWith({
      profiles: [{ ...makeProfile(), idleCloseMinutes: 1 }],
      standingInstructions: "",
    });
  });

  it("refuses minutes that are not a number, and never shows the form's own NaN", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const idleField = dom.container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Close idle children after minutes"]',
    );
    if (!idleField) throw new Error("the idle-close minutes field did not render");

    // A number the field cannot hold: it parses to Infinity, so the draft
    // must carry what the human typed (the refusal names it) rather than a
    // `String(NaN)` of the form's making — which no number input would
    // show either.
    await typeText(idleField, "1e999");
    expect(idleField.value).toBe("1e999");

    await act(async () => panelButton("Save").click());
    await act(async () => undefined);
    expect(agentProfilesSet).not.toHaveBeenCalled();
    const alert = dom.container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("whole number of minutes");
  });

  it("wires the idle-close hint to its input, the way every other field does", async () => {
    await renderAgentsPanel({ profiles: [makeProfile()], standingInstructions: "" });

    await act(async () => rowButton("Explorer", "Edit").click());
    await act(async () => undefined);
    const idleField = dom.container.querySelector<HTMLInputElement>(
      '.agent-inline-editor input[aria-label="Close idle children after minutes"]',
    );
    if (!idleField) throw new Error("the idle-close minutes field did not render");
    // The three sentences that carry the field's whole meaning — the
    // default, the cap and what the close waits for — are read out with the
    // control, not left beside it.
    expect(describedText(idleField)).toContain("30 by default");
    expect(describedText(idleField)).toContain("10080");
    expect(describedText(idleField)).toContain("nobody looking at it");
  });
});
