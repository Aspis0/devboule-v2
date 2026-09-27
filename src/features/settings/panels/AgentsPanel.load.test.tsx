// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const { agentsPanelTauriMock } = await import("./agentsPanelTestMocks");
  return agentsPanelTauriMock(await importOriginal());
});

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

import { agentProfilesGet, agentProfilesSet, daemonStatus } from "../../../lib/tauri";
import type { AgentProfilesReply } from "../../../types/ipc";
import { AgentProfilesPanel } from "./AgentsPanel";
import {
  dom,
  useAgentsPanelDom,
  daemonStatusWith,
  makeProfile,
  renderAgentsPanel,
  renderAgentsPanelLoading,
  renderAgentsPanelErrored,
  typeText,
} from "./agentsPanelTestHarness";
import { profileRows, rowByName, tickBox, rowButton, panelButton } from "./agentsPanelTestQueries";

describe("Settings agents panel — load, locks and refetch", () => {
  useAgentsPanelDom(() => []);

  it("hides the section and never fetches when the daemon lacks agent_profiles", async () => {
    // The module mock's default daemonStatus advertises tool_policy but not
    // agent_profiles: an older daemon. The tab still navigates; the section
    // is absent, not disabled and not an error.
    dom.root = createRoot(dom.container);
    await act(async () => dom.root!.render(<AgentProfilesPanel />));
    await act(async () => undefined);
    await act(async () => undefined);

    expect(agentProfilesGet).not.toHaveBeenCalled();
    expect(dom.container.querySelector(".agent-profiles")).toBeNull();
    expect(dom.container.textContent).not.toContain("Agents may create this");
    expect(dom.container.querySelector('[role="alert"]')).toBeNull();
  });

  it("locks every control while the document is in flight", async () => {
    await renderAgentsPanelLoading();

    expect(dom.container.textContent).toContain("Loading agent profiles…");
    const controls = dom.container.querySelectorAll<HTMLInputElement | HTMLButtonElement>(
      ".agent-profiles input, .agent-profiles textarea, .agent-profiles button",
    );
    expect(controls.length).toBeGreaterThan(0);
    for (const control of controls) expect(control.disabled).toBe(true);
    expect(agentProfilesSet).not.toHaveBeenCalled();
  });

  it("renders the profiles in the order the daemon returned", async () => {
    // Deliberately not alphabetical: the human's order is the feature.
    await renderAgentsPanel({
      profiles: [makeProfile({ id: "b", name: "Beta" }), makeProfile({ id: "a", name: "Alpha" })],
      standingInstructions: "",
    });

    expect(profileRows().map((row) => row.querySelector(".profile-name")?.textContent)).toEqual([
      "Beta",
      "Alpha",
    ]);
    expect(rowByName("Beta").querySelector<HTMLElement>(".profile-meta")?.textContent).toBe(
      "grok · grok-4 · ask",
    );
    // The edges are where a reorder bug would show: the first row cannot move
    // up and the last cannot move down.
    const firstUp = rowByName("Beta").querySelector<HTMLButtonElement>(
      "button[aria-label='Move Beta up']",
    );
    const lastDown = rowByName("Alpha").querySelector<HTMLButtonElement>(
      "button[aria-label='Move Alpha down']",
    );
    expect(firstUp?.disabled).toBe(true);
    expect(lastDown?.disabled).toBe(true);
  });

  it("ends a failed load in a retryable state instead of loading forever", async () => {
    await renderAgentsPanelErrored();

    // Terminal state: the daemon's sentence and a Retry, no loading line.
    expect(dom.container.textContent).not.toContain("Loading agent profiles…");
    expect(dom.container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    const retry = panelButton("Retry");

    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [makeProfile()], standingInstructions: "" },
    });
    await act(async () => retry.click());
    await act(async () => undefined);

    expect(agentProfilesGet).toHaveBeenCalledTimes(2);
    expect(profileRows()).toHaveLength(1);
    expect(dom.container.querySelector('[role="alert"]')).toBeNull();
  });

  it("applies a refetch after a write, so a restarted daemon's store replaces the stale panel", async () => {
    vi.useFakeTimers();
    try {
      await renderAgentsPanel({
        profiles: [makeProfile()],
        standingInstructions: "",
      });

      // The write is confirmed with a read-back (the panel adopts the stored
      // document, ids included): the store holds the tick.
      vi.mocked(agentProfilesGet).mockResolvedValueOnce({
        document: {
          profiles: [{ ...makeProfile(), enabledForAgents: true }],
          standingInstructions: "",
        },
      });

      // A write puts the sequence past zero; a daemon restart then flips the
      // handshake capability off and back on, re-running the load effect
      // while the panel stays mounted.
      await act(async () => tickBox("Explorer").click());
      await act(async () => undefined);
      expect(tickBox("Explorer").checked).toBe(true);

      // A draft typed after the write, never saved: the fresh load must
      // release it, so the box shows the restarted store's instructions.
      const fieldBefore = dom.container.querySelector<HTMLTextAreaElement>(
        ".agent-standing textarea",
      );
      if (!fieldBefore) throw new Error("standing instructions field did not render");
      await typeText(fieldBefore, "typed against the old daemon");

      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith(["ping", "status", "sessions", "journal", "typed_permissions", "devices"]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);

      // The daemon came back with an emptied store — the quarantined-file
      // direction — and the panel must take that truth, not keep the stale
      // optimistic document from before the restart.
      vi.mocked(agentProfilesGet).mockResolvedValueOnce({
        document: { profiles: [], standingInstructions: "fresh from the restarted store" },
      });
      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith([
          "ping",
          "status",
          "sessions",
          "journal",
          "typed_permissions",
          "devices",
          "agent_profiles",
        ]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);

      // Three reads in total: the load, the write's read-back, and the
      // restarted store's refetch.
      expect(agentProfilesGet).toHaveBeenCalledTimes(3);
      expect(profileRows()).toHaveLength(0);
      // A fresh load also releases any draft: the box reads the new store.
      const field = dom.container.querySelector<HTMLTextAreaElement>(".agent-standing textarea");
      expect(field?.value).toBe("fresh from the restarted store");
    } finally {
      vi.useRealTimers();
    }
  });

  it("holds the inline editor under the busy lock so a second write cannot start", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile(), makeProfile({ id: "profile-2", name: "Coder" })],
      standingInstructions: "",
    });

    // The editor is opened BEFORE any write, so its Save button exists while
    // another row's write is still in flight — the hole the lock closes.
    await act(async () => rowButton("Coder", "Edit").click());
    await act(async () => undefined);
    const save = panelButton("Save");

    vi.mocked(agentProfilesSet).mockImplementationOnce(() => new Promise<void>(() => undefined));
    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    expect(save.disabled).toBe(true);
    // Even a dispatched click cannot start a second write while the first is
    // in flight: React does not invoke onClick on a disabled button.
    await act(async () => save.click());
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
  });

  it("keeps the panel locked until the read-back lands, so no write can re-send an empty id", async () => {
    await renderAgentsPanel({
      profiles: [makeProfile()],
      standingInstructions: "",
    });
    // The write will confirm, and its read-back — where the daemon's minted
    // ids are adopted — is armed and does not answer yet. This constructs
    // the window the cumulative audit's finding 3: the moment between the
    // write's confirmation and its read-back.
    let resolveReadBack: ((reply: AgentProfilesReply) => void) | undefined;
    vi.mocked(agentProfilesGet).mockImplementationOnce(
      () =>
        new Promise<AgentProfilesReply>((resolve) => {
          resolveReadBack = resolve;
        }),
    );

    await act(async () => tickBox("Explorer").click());
    await act(async () => undefined);

    // The write is confirmed but its read-back has not landed. Every writer
    // must still be locked: a write sent now would travel on the
    // pre-read-back document, re-send `id: ""` for a row the daemon has
    // already named, and its sequence would discard the very read-back that
    // was about to heal the panel. An earlier version released `busy`
    // before the read-back resolved; these assertions are what that got
    // wrong.
    expect(agentProfilesSet).toHaveBeenCalledTimes(1);
    expect(agentProfilesGet).toHaveBeenCalledTimes(2);
    expect(tickBox("Explorer").disabled).toBe(true);
    expect(panelButton("New profile").disabled).toBe(true);

    // The read-back lands: the window closes, the minted id is adopted,
    // and the panel is writable again.
    resolveReadBack?.({
      document: {
        profiles: [makeProfile({ id: "minted-1", enabledForAgents: true })],
        standingInstructions: "",
      },
    });
    await act(async () => undefined);

    expect(tickBox("Explorer").disabled).toBe(false);
    expect(panelButton("New profile").disabled).toBe(false);
    expect(tickBox("Explorer").checked).toBe(true);
  });

  it("does not adopt a store fetch that raced a write still in flight", async () => {
    vi.useFakeTimers();
    try {
      await renderAgentsPanel({
        profiles: [makeProfile()],
        standingInstructions: "",
      });

      // A write whose fate is still open: the optimistic tick is on screen,
      // the daemon has not answered.
      let rejectSet!: (cause: unknown) => void;
      vi.mocked(agentProfilesSet).mockImplementationOnce(
        () =>
          new Promise<void>((_resolve, reject) => {
            rejectSet = reject;
          }),
      );
      await act(async () => tickBox("Explorer").click());
      await act(async () => undefined);
      expect(tickBox("Explorer").checked).toBe(true);

      // While that write is in flight, a daemon restart flips the
      // capability off and back on, re-running the load effect. Its reply
      // is the store's pre-write truth — the reply that must adopt nothing.
      vi.mocked(agentProfilesGet).mockResolvedValueOnce({
        document: {
          profiles: [makeProfile({ note: "raced the write" })],
          standingInstructions: "",
        },
      });
      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith(["ping", "status", "sessions", "journal", "typed_permissions", "devices"]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith([
          "ping",
          "status",
          "sessions",
          "journal",
          "typed_permissions",
          "devices",
          "agent_profiles",
        ]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);

      // The racing fetch was issued and adopted nothing: the optimistic
      // document still owns the panel. The old guard compared sequence
      // numbers only, so this reply WAS adopted here — and the write's
      // revert then clobbered it — because the guard never asked whether a
      // write was in flight when the fetch started.
      expect(agentProfilesGet).toHaveBeenCalledTimes(2);
      expect(tickBox("Explorer").checked).toBe(true);
      expect(dom.container.textContent).not.toContain("raced the write");

      // The write then refuses: the revert restores exactly what the human
      // was seeing, under the error — with no adopted reply in between.
      await act(async () => {
        rejectSet({ code: "io", message: "profile file unwritable" });
      });
      await act(async () => undefined);
      expect(tickBox("Explorer").checked).toBe(false);
      expect(dom.container.querySelector('[role="alert"]')?.textContent).toContain(
        "A system or file operation failed on this machine.",
      );
    } finally {
      vi.useRealTimers();
    }
  });
});
