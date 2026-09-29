// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const { providersTauriMock } = await import("./providersPanelTestMocks");
  return providersTauriMock(await importOriginal());
});

vi.mock("../../workspace/workspaceSessions", async (importOriginal) => {
  const { workspaceSessionsMock } = await import("./providersPanelTestMocks");
  return workspaceSessionsMock(await importOriginal());
});

import {
  daemonStatus,
  providerSetEnabled,
  providersList,
  providersRefresh,
  toolPolicyGet,
  toolPolicySet,
} from "../../../lib/tauri";
import { ProvidersPanel } from "./ProvidersPanel";
import { useSettingsDaemon } from "../settingsDaemon";
import {
  daemonStatusWith,
  installedProvider,
  installProvidersPanelMockReset,
} from "./providersPanelTestSetup";

installProvidersPanelMockReset();

describe("tools switch wiring", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
    vi.mocked(toolPolicySet).mockReset();
    vi.mocked(toolPolicySet).mockImplementation(async () => undefined);
  });

  async function renderPanel() {
    root = createRoot(container);
    await act(async () => root.render(<ProvidersPanel />));
    await act(async () => undefined);
  }

  function toolSwitch(): HTMLButtonElement | null {
    return container.querySelector<HTMLButtonElement>(
      '.prov-row [role="switch"][aria-label^="Devboule tools for"]',
    );
  }

  function providerSwitch(): HTMLButtonElement | null {
    return container.querySelector<HTMLButtonElement>(
      '.prov-row [role="switch"][aria-label^="On for"]',
    );
  }

  it("shows an off provider as Off, keeps its tools switch disabled, and persists On", async () => {
    vi.mocked(daemonStatus).mockResolvedValue(
      daemonStatusWith(["ping", "status", "tool_policy", "provider.switches"]),
    );
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        installedProvider({
          enabled: false,
          tools: [{ name: "some_tool", description: "Something." }],
        }),
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const onSwitch = providerSwitch();
    if (!onSwitch) throw new Error("provider switch did not render");
    expect(container.textContent).toContain("Off");
    expect(container.textContent).toContain("Existing sessions keep running.");
    expect(onSwitch.getAttribute("aria-checked")).toBe("false");
    expect(toolSwitch()?.disabled).toBe(true);
    await act(async () => onSwitch.click());
    await act(async () => undefined);
    expect(providerSetEnabled).toHaveBeenCalledWith("grok", true);
    expect(providerSwitch()?.getAttribute("aria-checked")).toBe("true");
  });

  it("reverts the provider switch and reports a failed write", async () => {
    vi.mocked(daemonStatus).mockResolvedValue(
      daemonStatusWith(["ping", "status", "tool_policy", "provider.switches"]),
    );
    vi.mocked(providerSetEnabled).mockRejectedValueOnce({
      code: "io",
      message: "switch file unwritable",
    });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider()],
      unreadableDirs: 0,
    });
    await renderPanel();

    const onSwitch = providerSwitch();
    if (!onSwitch) throw new Error("provider switch did not render");
    await act(async () => onSwitch.click());
    await act(async () => undefined);

    expect(providerSetEnabled).toHaveBeenCalledWith("grok", false);
    expect(providerSwitch()?.getAttribute("aria-checked")).toBe("true");
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
  });

  it("hides provider switches when the daemon does not advertise the capability", async () => {
    vi.mocked(daemonStatus).mockResolvedValue(daemonStatusWith(["ping", "status", "tool_policy"]));
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider()],
      unreadableDirs: 0,
    });
    await renderPanel();

    expect(container.textContent).toContain("grok");
    expect(providerSwitch()).toBeNull();
    expect(providerSetEnabled).not.toHaveBeenCalled();
  });

  it("hides the switch and never fetches when the daemon lacks tool_policy", async () => {
    vi.mocked(daemonStatus).mockResolvedValueOnce(
      daemonStatusWith(["ping", "status", "sessions", "journal", "typed_permissions", "devices"]),
    );
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    expect(toolPolicyGet).not.toHaveBeenCalled();
    expect(toolSwitch()).toBeNull();
    expect(container.textContent).not.toContain("Devboule tools");
    expect(container.textContent).toContain("grok");
  });

  it("hides the switch for providers that serve no Devboule tools", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    expect(toolSwitch()).toBeNull();
  });

  it("writes the single boolean with an empty deny list through the panel", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    const master = toolSwitch();
    if (!master) throw new Error("switch did not render");
    expect(master.getAttribute("aria-checked")).toBe("true");
    await act(async () => master.click());
    await act(async () => undefined);

    expect(toolPolicySet).toHaveBeenCalledTimes(1);
    expect(toolPolicySet).toHaveBeenCalledWith("grok", false, []);
    expect(master.getAttribute("aria-checked")).toBe("false");
  });

  it("shows the legacy notice when the stored row still denies tools", async () => {
    vi.mocked(toolPolicyGet).mockResolvedValueOnce({
      policies: [{ providerId: "grok", enabled: null, disabledTools: ["old_tool"] }],
    });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    const master = toolSwitch();
    if (!master) throw new Error("switch did not render");
    expect(master.getAttribute("aria-checked")).toBe("true");
    expect(container.textContent).toMatch(/older setting/i);
    const turnOn = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === "Turn all on",
    );
    if (!turnOn) throw new Error("Turn-all-on did not render");
    await act(async () => turnOn.click());
    await act(async () => undefined);
    expect(toolPolicySet).toHaveBeenCalledWith("grok", null, []);
  });

  it("normalises the legacy denials on the first switch write", async () => {
    vi.mocked(toolPolicyGet).mockResolvedValueOnce({
      policies: [{ providerId: "grok", enabled: null, disabledTools: ["old_tool"] }],
    });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    const master = toolSwitch();
    if (!master) throw new Error("switch did not render");
    await act(async () => master.click());
    await act(async () => undefined);

    expect(toolPolicySet).toHaveBeenCalledWith("grok", false, []);
    expect(container.textContent).not.toMatch(/older setting/i);
  });

  it("reverts the switch and reports inside the row on rejection", async () => {
    vi.mocked(toolPolicySet).mockRejectedValueOnce({
      code: "io",
      message: "policy file unwritable",
    });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    const master = toolSwitch();
    if (!master) throw new Error("switch did not render");
    await act(async () => master.click());
    await act(async () => undefined);

    expect(master.getAttribute("aria-checked")).toBe("true");
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
  });

  it("ends a failed policy load in Retry instead of guessing", async () => {
    vi.mocked(toolPolicyGet).mockRejectedValueOnce({ code: "io", message: "pipe is gone" });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    const master = toolSwitch();
    if (!master) throw new Error("switch did not render");
    expect(master.disabled).toBe(true);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    const retry = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === "Retry",
    );
    if (!retry) throw new Error("Retry did not render");
    vi.mocked(toolPolicyGet).mockResolvedValueOnce({ policies: [] });
    await act(async () => retry.click());
    await act(async () => undefined);
    expect(toolPolicyGet).toHaveBeenCalledTimes(2);
    expect(toolSwitch()?.disabled).toBe(false);
  });

  it("labels sections with the shell's shared subheading, not a page rule", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        installedProvider(),
        {
          id: "codex-acp",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@agentclientprotocol/codex-acp",
        },
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const labels = container.querySelectorAll("h3.settings-subheading");
    expect(labels).toHaveLength(2);
    expect(container.querySelector(".prov-section-label")).toBeNull();
  });

  it("brings the legacy notice back when its normalising write is rejected", async () => {
    vi.mocked(toolPolicyGet).mockResolvedValueOnce({
      policies: [{ providerId: "grok", enabled: null, disabledTools: ["old_tool"] }],
    });
    vi.mocked(toolPolicySet).mockRejectedValueOnce({
      code: "io",
      message: "policy file unwritable",
    });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();
    expect(container.textContent).toMatch(/older setting/i);

    const master = toolSwitch();
    if (!master) throw new Error("switch did not render");
    await act(async () => master.click());
    await act(async () => undefined);

    // The write is rejected, so the stored denials are restored exactly —
    // and the notice returns with them instead of going silent.
    expect(master.getAttribute("aria-checked")).toBe("true");
    expect(container.textContent).toMatch(/older setting/i);
    expect(container.querySelector('[role="alert"]')).not.toBeNull();
  });

  it("adopts a reconnect refetch over a stale optimistic row", async () => {
    vi.useFakeTimers();
    try {
      vi.mocked(toolPolicyGet).mockResolvedValueOnce({
        policies: [{ providerId: "grok", enabled: null, disabledTools: ["old_tool"] }],
      });
      vi.mocked(providersList).mockResolvedValueOnce({
        providers: [
          installedProvider({ tools: [{ name: "some_tool", description: "Something." }] }),
        ],
        unreadableDirs: 0,
      });
      await renderPanel();
      expect(container.textContent).toMatch(/older setting/i);

      // The daemon restarts without tool_policy, then comes back with a
      // clean stored row: the refetch wins and the stale denial is gone.
      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith(["ping", "status", "sessions", "journal", "devices"]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);
      expect(toolSwitch()).toBeNull();

      vi.mocked(toolPolicyGet).mockResolvedValueOnce({
        policies: [{ providerId: "grok", enabled: true, disabledTools: [] }],
      });
      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith(["ping", "status", "sessions", "journal", "devices", "tool_policy"]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);

      expect(toolPolicyGet).toHaveBeenCalledTimes(2);
      expect(toolSwitch()?.getAttribute("aria-checked")).toBe("true");
      expect(container.textContent).not.toMatch(/older setting/i);
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows a refetch failure beside working switches instead of hiding it", async () => {
    vi.mocked(toolPolicyGet).mockResolvedValueOnce({ policies: [] });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();
    expect(toolSwitch()?.disabled).toBe(false);
    expect(container.querySelector('[role="alert"]')).toBeNull();

    // A refresh drops every tool-bearing provider, then brings them back
    // while the refetch fails: last-known rows stay usable, and the error
    // is shown with a Retry instead of sitting invisibly in the store.
    vi.mocked(providersRefresh).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [] })],
      unreadableDirs: 0,
    });
    const refresh = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!refresh) throw new Error("Refresh button did not render");
    await act(async () => refresh.click());
    await act(async () => undefined);
    expect(toolSwitch()).toBeNull();

    vi.mocked(toolPolicyGet).mockRejectedValueOnce({ code: "io", message: "pipe is gone" });
    vi.mocked(providersRefresh).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await act(async () => refresh.click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    expect(toolSwitch()?.disabled).toBe(false);
  });

  it("locks the tool switches and banners while the tool policy is failed closed", async () => {
    // The daemon denies every restrictable tool while `toolPolicyError` is
    // set: the banner shows the daemon's sentence and the switch locks, so
    // the panel never renders the deny-all as allowed and clickable.
    vi.mocked(daemonStatus).mockResolvedValue({
      ...daemonStatusWith(["ping", "status", "tool_policy", "provider.switches"]),
      toolPolicyError: "tool-policies.json exists but holds no policy",
    });
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ tools: [{ name: "some_tool", description: "Something." }] })],
      unreadableDirs: 0,
    });
    await renderPanel();

    const banner = container.querySelector('[role="alert"]');
    if (!banner) throw new Error("failed-closed banner did not render");
    expect(banner.textContent).toContain("tool-policies.json exists but holds no policy");
    const master = toolSwitch();
    if (!master) throw new Error("switch did not render");
    expect(master.disabled).toBe(true);
  });

  it("keeps the banner visible across a disconnect and releases it on a clean status", async () => {
    // Restarting takes the pipe down: the panel must keep describing the
    // deny-all until a fresh status clears it. That is the whole guarantee —
    // the tool section hides with the dropped capabilities, so there is no
    // toggle to assert on while disconnected.
    vi.useFakeTimers();
    try {
      vi.mocked(daemonStatus).mockResolvedValue({
        ...daemonStatusWith(["ping", "status", "tool_policy", "provider.switches"]),
        toolPolicyError: "tool-policies.json exists but holds no policy",
      });
      vi.mocked(providersList).mockResolvedValueOnce({
        providers: [
          installedProvider({ tools: [{ name: "some_tool", description: "Something." }] }),
        ],
        unreadableDirs: 0,
      });
      await renderPanel();
      expect(toolSwitch()?.disabled).toBe(true);

      vi.mocked(daemonStatus).mockRejectedValue(new Error("pipe is gone"));
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);

      const banner = container.querySelector('[role="alert"]');
      if (!banner) throw new Error("failed-closed banner did not survive the disconnect");
      expect(banner.textContent).toContain("tool-policies.json exists but holds no policy");
      expect(toolSwitch()).toBeNull();

      vi.mocked(daemonStatus).mockResolvedValue(
        daemonStatusWith(["ping", "status", "tool_policy", "provider.switches"]),
      );
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);
      expect(container.textContent).not.toContain("tool-policies.json exists but holds no policy");
      expect(toolSwitch()?.disabled).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not re-render subscribers on every failed poll", async () => {
    // The disconnect arm preserves snapshot identity like the timeout arm:
    // one emit per outage, not one per poll.
    vi.useFakeTimers();
    try {
      let renders = 0;
      function Probe() {
        const daemon = useSettingsDaemon();
        renders += 1;
        return <span data-testid="probe">{daemon.toolPolicyError ?? "none"}</span>;
      }
      vi.mocked(daemonStatus).mockResolvedValue({
        ...daemonStatusWith(["ping", "status"]),
        toolPolicyError: "tool-policies.json exists but holds no policy",
      });
      root = createRoot(container);
      await act(async () => root.render(<Probe />));
      await act(async () => undefined);

      vi.mocked(daemonStatus).mockRejectedValue(new Error("pipe is gone"));
      await act(async () => {
        vi.advanceTimersByTime(2_100);
      });
      await act(async () => undefined);
      const afterFirstReject = renders;
      await act(async () => {
        vi.advanceTimersByTime(4_200);
      });
      await act(async () => undefined);
      expect(renders).toBe(afterFirstReject);
    } finally {
      vi.useRealTimers();
    }
  });
});
