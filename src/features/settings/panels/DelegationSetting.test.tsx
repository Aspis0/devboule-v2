// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../lib/tauri")>();
  return {
    ...actual,
    isCommandError: vi.fn(
      (error: unknown) =>
        typeof error === "object" && error !== null && "code" in error && "message" in error,
    ),
    daemonStatus: vi.fn(async () => ({
      state: "connected",
      pid: 1,
      instanceId: "settings-test",
      protocolVersion: 4,
      clients: 1,
      capabilities: [
        "ping",
        "status",
        "sessions",
        "journal",
        "typed_permissions",
        "devices",
        "tool_policy",
      ],
      message: null,
    })),
    journalRetentionGet: vi.fn(),
    journalRetentionSet: vi.fn(),
    journalUsage: vi.fn(),
    // The General panel's close-behavior and notification-sound rows read
    // and write their own surface settings.
    surfaceSettingsGet: vi.fn(async () => ({ status: "absent" })),
    surfaceSettingsSet: vi.fn(async () => undefined),
    projectAdd: vi.fn(),
    projectsList: vi.fn(async () => []),
    providersList: vi.fn(async () => ({ providers: [], unreadableDirs: 0 })),
    providersRefresh: vi.fn(async () => ({ providers: [], unreadableDirs: 0 })),
    providerUpdate: vi.fn(async () => ({ ok: true, exitCode: 0, log: "" })),
    toolPolicyGet: vi.fn(async () => ({ policies: [] })),
    toolPolicySet: vi.fn(async () => undefined),
    agentProfilesGet: vi.fn(async () => ({
      document: {
        profiles: [],
        standingInstructions: "",
      } as AgentProfilesDocument,
    })),
    agentProfilesSet: vi.fn(async () => undefined),
    // No default answer: a vocabulary query only ever leaves the app when the
    // handshake advertised `provider_vocabulary`, and the tests that arm it
    // queue their own replies.
    providerVocabularyGet: vi.fn(),
    // The delegation pair: never called unless the handshake advertised
    // `permission_delegation`, and every test arms its own replies.
    delegationGet: vi.fn(),
    delegationSet: vi.fn(async () => undefined),
    workspacesList: vi.fn(async () => []),
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("../../oracle/OraclePanel", () => ({
  OraclePanel: () => <div>Oracle mock</div>,
}));

import { agentProfilesGet, daemonStatus, delegationGet, delegationSet } from "../../../lib/tauri";
import type { AgentProfilesDocument, DaemonStatus } from "../../../types/ipc";
import { DelegationSetting, SettingsSurface } from "../SettingsSurface";
import { createDelegationController } from "../../../lib/delegation";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
describe("DelegationSetting - the switch beside the profiles", () => {
  let container: HTMLDivElement;
  let root: Root | null = null;

  const DELEGATION_DAEMON = [
    "ping",
    "status",
    "sessions",
    "journal",
    "typed_permissions",
    "devices",
    "agent_profiles",
    "permission_delegation",
  ];

  function daemonStatusWithDelegated(capabilities: string[]): DaemonStatus {
    return {
      state: "connected",
      pid: 1,
      instanceId: "settings-test",
      protocolVersion: 4,
      clients: 1,
      capabilities,
      message: null,
    };
  }

  function mountDelegation(capabilities: string[]) {
    vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWithDelegated(capabilities));
    root = createRoot(container);
    const controller = createDelegationController({
      get: delegationGet as unknown as () => Promise<{
        enabled: boolean;
        source: "file" | "default" | "quarantined";
      }>,
      set: delegationSet as unknown as (enabled: boolean) => Promise<void>,
    });
    act(() => {
      root!.render(<DelegationSetting controller={controller} />);
    });
    return controller;
  }

  async function settle() {
    await act(async () => undefined);
    await act(async () => undefined);
  }

  function theSwitch() {
    const input = container.querySelector<HTMLInputElement>(
      'input[aria-label="Let agents answer their children\'s cards"]',
    );
    if (input === null) throw new Error("delegation switch did not render");
    return input;
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(delegationGet).mockReset();
    vi.mocked(delegationSet).mockReset();
    vi.mocked(delegationSet).mockResolvedValue(undefined);
    // Reset the app's shared controller between tests: the last test's
    // answer must not be this test's starting point.
    vi.mocked(delegationGet).mockResolvedValue({ enabled: false, source: "default" });
  });

  afterEach(async () => {
    if (root !== null) {
      await act(async () => root!.unmount());
      root = null;
    }
    container.remove();
    vi.clearAllMocks();
  });

  it("never asks a daemon that does not advertise the capability, and names that absence", async () => {
    vi.mocked(delegationGet).mockClear();
    mountDelegation(DELEGATION_DAEMON.slice(0, 7));
    await settle();

    const note = container.querySelector(".agent-delegation-unavailable");
    expect(note?.textContent).toContain("permission_delegation");
    expect(note?.textContent).toContain("older than this app");
    // The switch is not drawn and the request is never sent - the section is
    // not broken, it is absent, and the absence has a name.
    expect(container.querySelector(".agent-delegation")).toBeNull();
    expect(delegationGet).not.toHaveBeenCalled();
  });

  it("fetches on mount when the handshake advertised the capability", async () => {
    vi.mocked(delegationGet).mockResolvedValue({ enabled: false, source: "default" });
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    expect(delegationGet).toHaveBeenCalledTimes(1);
    expect(theSwitch().checked).toBe(false);
    expect(theSwitch().disabled).toBe(false);
    // `default` is "never configured", not "off": no human said anything yet.
    expect(container.querySelector(".agent-delegation-source")?.textContent).toBe(
      "Never configured",
    );
  });

  it("re-reads when the daemon restarts — a cached answer may not outlive its daemon", async () => {
    // Audit 3 F2: the setting was read at mount only, so a daemon restart
    // that reloads `delegation.json` — the writer the app's own `source:
    // "file"` sentence names — left the panel stale forever. Here the poll's
    // next answer reports a fresh daemon instance with NO disconnected gap;
    // the effect must re-ask on the instance's identity alone.
    vi.useFakeTimers();
    try {
      const statusFor = (instanceId: string): DaemonStatus => ({
        state: "connected",
        pid: 1,
        instanceId,
        protocolVersion: 4,
        clients: 1,
        capabilities: DELEGATION_DAEMON,
        message: null,
      });
      const answers: DaemonStatus[] = [statusFor("instance-a"), statusFor("instance-b")];
      vi.mocked(daemonStatus).mockImplementation(() => {
        const next = answers.shift();
        return Promise.resolve(next ?? statusFor("instance-b"));
      });
      vi.mocked(delegationGet)
        .mockResolvedValueOnce({ enabled: false, source: "file" })
        .mockResolvedValueOnce({ enabled: true, source: "file" });

      root = createRoot(container);
      const controller = createDelegationController({
        get: delegationGet as unknown as () => Promise<{
          enabled: boolean;
          source: "file" | "default" | "quarantined";
        }>,
        set: delegationSet as unknown as (enabled: boolean) => Promise<void>,
      });
      await act(async () => {
        root!.render(<DelegationSetting controller={controller} />);
      });
      await act(async () => undefined);

      // The first instance answered: off, and the human's corrective control
      // (the switch) reads that value.
      expect(delegationGet).toHaveBeenCalledTimes(1);
      expect(theSwitch().checked).toBe(false);

      // The restart: a new instance, same capabilities, no gap observed.
      await act(async () => {
        vi.advanceTimersByTime(2_000);
      });
      await act(async () => undefined);

      // The panel followed its new daemon: it read the fresh answer (on — a
      // human flipped delegation.json while the old daemon was down) instead
      // of keeping the dead instance's word.
      expect(delegationGet).toHaveBeenCalledTimes(2);
      expect(theSwitch().checked).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("states the blast radius and the global scope - the copy without which there is no consent", async () => {
    vi.mocked(delegationGet).mockResolvedValue({ enabled: false, source: "default" });
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    const note = container.querySelector(".agent-profile-tick-note")?.textContent ?? "";
    expect(note).toContain("a write, a command, a network call");
    expect(note).toContain("every child of every agent");
    expect(note).toContain("not the one you see");
  });

  it("renders a quarantined file as damaged - neither never-configured nor deliberately off", async () => {
    vi.mocked(delegationGet).mockResolvedValue({ enabled: false, source: "quarantined" });
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    const source = container.querySelector(".agent-delegation-source")?.textContent ?? "";
    expect(source).toBe("Settings file was damaged — delegation reads off");
    expect(source).not.toBe("Never configured");
    expect(source).not.toBe("Off");
  });

  it("renders a FIFTH source value as its own visible sentence, never a blank status line", async () => {
    // The cast builds the value a newer daemon could deliver and TypeScript
    // cannot predict; a plain Record lookup would yield undefined and render
    // an empty <p role="status"> — a status line that says nothing.
    const fifthSource = "paused" as unknown as "file" | "default" | "quarantined";
    vi.mocked(delegationGet).mockResolvedValue({ enabled: false, source: fifthSource });
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    const source = container.querySelector(".agent-delegation-source");
    expect(source).not.toBeNull();
    const text = source?.textContent ?? "";
    expect(text).not.toBe("");
    expect(text).toContain("cannot name");
  });

  it("names the unknown while the stored answer is in flight — and the CONTROL looks unknown, not off", async () => {
    vi.mocked(delegationGet).mockImplementation(() => new Promise(() => undefined));
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    // The control itself must look unknown (audit 3 F5 — the re-audit's fix
    // corrected the sentence beside the control but left the switch reading
    // as a definite off): the dash paints `indeterminate`, the aria state is
    // mixed, and the unknown treatment marks the control it locks. What the
    // old assertion pinned — `checked === false` — is still true (a
    // dash-painting checkbox must not claim a checkedness), but it is no
    // longer the state a human reads.
    expect(theSwitch().checked).toBe(false);
    expect(theSwitch().indeterminate).toBe(true);
    expect(theSwitch().getAttribute("aria-checked")).toBe("mixed");
    expect(theSwitch().className).toContain("agent-delegation-switch-unknown");
    expect(theSwitch().disabled).toBe(true);
    const status = container.querySelector(".agent-delegation-source");
    expect(status?.textContent).toBe("Reading the stored answer…");
    expect(status?.textContent).not.toBe("Off");
    expect(status?.textContent).not.toBe("Never configured");
  });

  it("names the unknown after a failed load too — and the CONTROL looks unknown, not off", async () => {
    vi.mocked(delegationGet).mockRejectedValue(new Error("the daemon is unreachable"));
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    // The unknown state is the unknown state wherever it comes from: a read
    // that never answers and one that fails render the same honest control.
    expect(theSwitch().checked).toBe(false);
    expect(theSwitch().indeterminate).toBe(true);
    expect(theSwitch().getAttribute("aria-checked")).toBe("mixed");
    expect(theSwitch().disabled).toBe(true);
    const status = container.querySelector(".agent-delegation-source")?.textContent ?? "";
    expect(status).toContain("could not be read");
    expect(status).toContain("not an off");
    // The way back stays on the panel beside the named state.
    expect(container.querySelector(".settings-device-action")?.textContent).toBe("Retry");
  });

  it("reports a reply that contradicts itself instead of dressing it up (re-audit F11)", async () => {
    // `quarantined` reads off with a sane daemon; a reply pairing it with
    // `enabled: true` is inconsistent, and the render must name the
    // inconsistency rather than print "delegation reads off" beside a
    // checked switch — a sentence inventing a coherence the reply lacks.
    vi.mocked(delegationGet).mockResolvedValue({ enabled: true, source: "quarantined" });
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    expect(theSwitch().checked).toBe(true);
    const status = container.querySelector(".agent-delegation-source")?.textContent ?? "";
    expect(status).toContain("contradicts itself");
    expect(status).not.toContain("reads off");
  });

  it("reports the contradiction for never-configured beside an on switch too", async () => {
    vi.mocked(delegationGet).mockResolvedValue({ enabled: true, source: "default" });
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    expect(theSwitch().checked).toBe(true);
    const status = container.querySelector(".agent-delegation-source")?.textContent ?? "";
    expect(status).toContain("contradicts itself");
    expect(status).not.toBe("Never configured");
  });

  it("the source sentence follows a successful write instead of contradicting the switch", async () => {
    vi.mocked(delegationGet).mockResolvedValue({ enabled: false, source: "quarantined" });
    mountDelegation(DELEGATION_DAEMON);
    await settle();
    expect(container.querySelector(".agent-delegation-source")?.textContent).toBe(
      "Settings file was damaged — delegation reads off",
    );

    await act(async () => theSwitch().click());
    await settle();
    // The switch reads ON and the sentence says what the daemon now holds —
    // not the stale "delegation reads off" from before the write.
    expect(theSwitch().checked).toBe(true);
    expect(container.querySelector(".agent-delegation-source")?.textContent).toBe("On");
  });

  it("writes through the controller when toggled, and shows the optimistic value", async () => {
    vi.mocked(delegationGet).mockResolvedValue({ enabled: false, source: "file" });
    mountDelegation(DELEGATION_DAEMON);
    await settle();

    await act(async () => theSwitch().click());
    expect(delegationSet).toHaveBeenCalledWith(true);
    expect(theSwitch().checked).toBe(true);
  });

  it("reverts the switch, reports the refusal, then asks the daemon what it actually holds", async () => {
    // Audit 3 F1: a rejection says the transport failed, not what the daemon
    // holds. The surface reports the refusal AND follows the re-read that
    // settles the doubt — here the re-read is gated, so the sentence's
    // standing time is under the test's hand.
    let releaseReread!: () => void;
    const reread = new Promise<{ enabled: boolean; source: "file" | "default" | "quarantined" }>(
      (resolve) => {
        releaseReread = () => resolve({ enabled: false, source: "default" });
      },
    );
    vi.mocked(delegationGet)
      .mockResolvedValueOnce({ enabled: false, source: "default" })
      .mockImplementationOnce(() => reread);
    mountDelegation(DELEGATION_DAEMON);
    await settle();
    vi.mocked(delegationSet).mockRejectedValueOnce(new Error("the store refused the write"));

    await act(async () => theSwitch().click());
    expect(delegationSet).toHaveBeenCalledWith(true);
    expect(theSwitch().checked).toBe(false);
    expect(container.querySelector(".device-error")?.textContent).toBe(
      "the store refused the write",
    );

    // The daemon answers the re-read: it holds what the panel fell back to,
    // so the panel is consistent again and the refusal sentence is gone.
    releaseReread();
    await settle();
    expect(container.querySelector(".device-error")).toBeNull();
    expect(theSwitch().checked).toBe(false);
    expect(container.querySelector(".agent-delegation-source")?.textContent).toBe(
      "Never configured",
    );
  });

  it("renders beside the profiles in the Agents tab - one consent surface", async () => {
    vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWithDelegated(DELEGATION_DAEMON));
    vi.mocked(agentProfilesGet).mockResolvedValueOnce({
      document: { profiles: [], standingInstructions: "" },
    });
    root = createRoot(container);
    await act(async () => root!.render(<SettingsSurface />));
    await act(async () => undefined);
    const tab = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-agents']",
    );
    if (!tab) throw new Error("Agents tab did not render");
    await act(async () => tab.click());
    await act(async () => undefined);

    const panel = container.querySelector("#settings-panel-agents");
    expect(panel).not.toBeNull();
    // The switch section and the profile list are siblings in the same panel.
    expect(panel?.querySelector(".agent-delegation")).not.toBeNull();
    expect(panel?.querySelector(".agent-profile-list")).not.toBeNull();
    // The armed capability makes the app actually ask.
    expect(delegationGet).toHaveBeenCalled();
  });
});
