// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn(async () => "not read yet"),
}));

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../lib/tauri")>();
  return {
    ...actual,
    daemonDiagnostics: vi.fn(),
    daemonStatus: vi.fn(),
  };
});

import { getVersion } from "@tauri-apps/api/app";
import { daemonDiagnostics, daemonStatus } from "../../../lib/tauri";
import type { DaemonDiagnostics, DaemonStatus } from "../../../types/ipc";
import { settingsPageById } from "../settingsMenu";
import diagnosticsFixture from "../../../../crates/devboule-daemon/fixtures/diagnostics-report.json";
import { AboutPanel } from "./AboutPanel";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// The committed fixture the Rust diagnostics round-trip test keeps honest:
// the page reads the real wire, not a hand-built shape.
const diagnosticsReport = diagnosticsFixture as DaemonDiagnostics;

const MAPPED_OFFLINE = "The connection to the agent daemon was lost. Devboule is reconnecting.";
const RAW_OFFLINE = "transport socket closed unexpectedly";

function offlineStatus(): DaemonStatus {
  return {
    state: "disconnected",
    pid: null,
    instanceId: null,
    protocolVersion: null,
    clients: null,
    capabilities: [],
    message: "daemon unreachable",
  };
}

function liveStatus(): DaemonStatus {
  return {
    state: "connected",
    pid: 1,
    instanceId: "about-daemon",
    protocolVersion: 18,
    clients: 1,
    capabilities: [],
    message: null,
  };
}

describe("AboutPanel", () => {
  let container: HTMLDivElement;
  let root: Root;

  function sectionOf(title: string): HTMLElement {
    const section = Array.from(
      container.querySelectorAll<HTMLElement>("[data-settings-section]"),
    ).find(
      (candidate) => candidate.querySelector(".settings-section-label")?.textContent === title,
    );
    expect(section, `the ${title} section did not render`).not.toBeUndefined();
    return section as HTMLElement;
  }

  function valueIn(section: HTMLElement, label: string): string | null {
    const row = Array.from(section.querySelectorAll("[data-settings-row]")).find(
      (candidate) => candidate.querySelector(".settings-row-title")?.textContent === label,
    );
    return row?.querySelector(".settings-row-control")?.textContent ?? null;
  }

  async function renderPanel(): Promise<void> {
    root = createRoot(container);
    await act(async () => root.render(<AboutPanel />));
    await act(async () => undefined);
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(getVersion).mockResolvedValue("7.7.7");
    vi.mocked(daemonDiagnostics).mockResolvedValue(diagnosticsReport);
    vi.mocked(daemonStatus).mockResolvedValue(liveStatus());
  });

  afterEach(async () => {
    if (root !== undefined) await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it("shows the app version the Tauri call reports, never a literal", async () => {
    await renderPanel();
    expect(vi.mocked(getVersion)).toHaveBeenCalledTimes(1);
    const app = sectionOf("This app");
    expect(valueIn(app, "Version")).toBe("7.7.7");
    expect(app.textContent).not.toContain("0.1.0");
  });

  it("shows the daemon and protocol versions from diagnostics", async () => {
    await renderPanel();
    const daemon = sectionOf("Daemon");
    expect(valueIn(daemon, "Version")).toBe(diagnosticsReport.daemon.version);
    expect(valueIn(daemon, "Protocol version")).toBe(
      String(diagnosticsReport.daemon.protocolVersion),
    );
  });

  it("shows the mapped sentence when diagnostics fail, never the raw text", async () => {
    vi.mocked(daemonDiagnostics).mockRejectedValue({
      code: "connection_lost",
      message: RAW_OFFLINE,
    });
    await renderPanel();
    const daemon = sectionOf("Daemon");
    expect(daemon.textContent).toContain(MAPPED_OFFLINE);
    expect(container.textContent).not.toContain(RAW_OFFLINE);
  });

  it("shows fresh versions when the connection comes back, without a remount", async () => {
    vi.useFakeTimers();
    try {
      vi.mocked(daemonStatus).mockResolvedValue(offlineStatus());
      vi.mocked(daemonDiagnostics).mockRejectedValue({
        code: "connection_lost",
        message: RAW_OFFLINE,
      });
      await renderPanel();
      expect(sectionOf("Daemon").textContent).toContain(MAPPED_OFFLINE);
      const panel = container.querySelector("#settings-panel-about");

      // The daemon answers the next status poll, so the connection returns.
      vi.mocked(daemonStatus).mockResolvedValue(liveStatus());
      vi.mocked(daemonDiagnostics).mockResolvedValue(diagnosticsReport);
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2000);
      });
      await act(async () => undefined);

      const daemon = sectionOf("Daemon");
      expect(valueIn(daemon, "Version")).toBe(diagnosticsReport.daemon.version);
      expect(daemon.textContent).not.toContain(MAPPED_OFFLINE);
      expect(container.querySelector("#settings-panel-about")).toBe(panel);
    } finally {
      vi.useRealTimers();
    }
  });

  it("says whose code is Apache-2.0 and where the other licenses are listed", async () => {
    await renderPanel();
    const license = sectionOf("License");
    expect(valueIn(license, "Devboule's own code")).toBe("Apache-2.0");
    expect(license.textContent).toContain(
      "Third-party components keep their own licenses, listed in THIRD_PARTY.md.",
    );
    expect(license.textContent).not.toContain("in the source");
  });
});

describe("the About page in the settings menu", () => {
  it("is available in the menu", () => {
    expect(settingsPageById("about").unavailable).not.toBe(true);
  });
});
