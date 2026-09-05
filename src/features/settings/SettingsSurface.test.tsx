// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/tauri", () => ({
  isCommandError: vi.fn(
    (error: unknown) =>
      typeof error === "object" && error !== null && "code" in error && "message" in error,
  ),
  journalRetentionGet: vi.fn(),
  journalRetentionSet: vi.fn(),
  journalUsage: vi.fn(),
  providersList: vi.fn(async () => ({ providers: [], unreadableDirs: 0 })),
}));

vi.mock("../oracle/OraclePanel", () => ({
  OraclePanel: () => <div>Oracle mock</div>,
}));

import {
  journalRetentionGet,
  journalRetentionSet,
  journalUsage,
  providersList,
} from "../../lib/tauri";
import type { JournalRetention } from "../../types/ipc";
import { SettingsSurface } from "./SettingsSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("Settings retention panel", () => {
  let container: HTMLDivElement;
  let root: Root;
  const persistedRetention: JournalRetention = {
    sessionMaxBytes: { value: 512 * 1024 * 1024, source: "default" },
    maxBytes: { value: 8 * 1024 * 1024 * 1024, source: "default" },
    maxSessions: { value: 10_000, source: "default" },
    maxAgeMs: { value: 0, source: "default" },
  };

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(journalUsage).mockResolvedValue({
      totalBytes: 12_345,
      sessionCount: 10_002,
      deletedByUser: 0,
      deletedByRetention: 3,
      unreclaimable: { bytesOver: 0, sessionsOver: 2, agedOut: 4 },
      limits: {
        snapshotEveryBytes: 65_536,
        sessionMaxBytes: 512 * 1024 * 1024,
        maxBytes: 8 * 1024 * 1024 * 1024,
        maxSessions: 10_000,
        maxAgeMs: 0,
      },
      perSession: [],
    });
    vi.mocked(journalRetentionGet).mockResolvedValue({
      sessionMaxBytes: { value: 512 * 1024 * 1024, source: "default" },
      maxBytes: { value: 8 * 1024 * 1024 * 1024, source: "default" },
      maxSessions: { value: 10_000, source: "default" },
      maxAgeMs: { value: 0, source: "default" },
    });
    vi.mocked(journalRetentionSet).mockResolvedValue(persistedRetention);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it("renders why retention is blocked and the measured counters", async () => {
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    const general = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-general']",
    );
    if (!general) throw new Error("General tab did not render");
    await act(async () => general.click());
    await act(async () => undefined);

    expect(container.textContent).toContain("Retention is blocked because");
    expect(container.textContent).toContain("2 sessions over the session limit");
    expect(container.textContent).toContain("4 sessions past the age limit");
    expect(container.textContent).toContain(
      "Lowering a limit takes effect immediately and can delete history.",
    );
  });

  it("requires an explicit zero instead of treating an empty field as no limit", async () => {
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    const general = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-general']",
    );
    if (!general) throw new Error("General tab did not render");
    await act(async () => general.click());
    await act(async () => undefined);
    const input = container.querySelector<HTMLInputElement>("input[aria-label='Maximum age']");
    if (!input) throw new Error("Maximum age input did not render");
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (!setValue) throw new Error("input value setter did not exist");
    setValue.call(input, "");
    await act(async () => input.dispatchEvent(new Event("input", { bubbles: true })));

    expect(journalRetentionSet).not.toHaveBeenCalled();
    expect(container.querySelector('[role="alert"]')?.textContent ?? "").toContain(
      "Enter 0 to disable a limit.",
    );
  });

  it("commits a complete value on blur instead of persisting each prefix", async () => {
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    const general = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-general']",
    );
    if (!general) throw new Error("General tab did not render");
    await act(async () => general.click());
    await act(async () => undefined);
    const input = container.querySelector<HTMLInputElement>("input[aria-label='Maximum sessions']");
    if (!input) throw new Error("Maximum sessions input did not render");
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (!setValue) throw new Error("input value setter did not exist");
    input.focus();
    setValue.call(input, "1");
    await act(async () => input.dispatchEvent(new Event("input", { bubbles: true })));
    setValue.call(input, "10");
    await act(async () => input.dispatchEvent(new Event("input", { bubbles: true })));
    expect(journalRetentionSet).not.toHaveBeenCalled();

    await act(async () => input.blur());
    await act(async () => undefined);
    expect(journalRetentionSet).toHaveBeenCalledTimes(1);
    expect(journalRetentionSet).toHaveBeenCalledWith({ maxSessions: 10 });
  });

  it("keeps a focused edit when a blur commit resolves late", async () => {
    let resolveCommit: ((retention: JournalRetention) => void) | undefined;
    vi.mocked(journalRetentionSet).mockReturnValueOnce(
      new Promise<JournalRetention>((resolve) => {
        resolveCommit = resolve;
      }),
    );
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    const general = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-general']",
    );
    if (!general) throw new Error("General tab did not render");
    await act(async () => general.click());
    await act(async () => undefined);
    const input = container.querySelector<HTMLInputElement>("input[aria-label='Maximum sessions']");
    if (!input) throw new Error("Maximum sessions input did not render");
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (!setValue) throw new Error("input value setter did not exist");
    input.focus();
    setValue.call(input, "12");
    await act(async () => input.dispatchEvent(new Event("input", { bubbles: true })));
    expect(journalRetentionSet).not.toHaveBeenCalled();
    await act(async () => input.blur());
    input.focus();
    setValue.call(input, "123");
    await act(async () => input.dispatchEvent(new Event("input", { bubbles: true })));
    resolveCommit?.({ ...persistedRetention, maxSessions: { value: 12, source: "user" } });
    await act(async () => undefined);
    expect(input.value).toBe("123");
  });

  it("restores the persisted value when a commit is rejected", async () => {
    vi.mocked(journalRetentionSet).mockRejectedValueOnce({
      code: "invalid_request",
      message: "rejected",
    });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    const general = container.querySelector<HTMLButtonElement>(
      "[aria-controls='settings-panel-general']",
    );
    if (!general) throw new Error("General tab did not render");
    await act(async () => general.click());
    await act(async () => undefined);
    const input = container.querySelector<HTMLInputElement>("input[aria-label='Maximum age']");
    if (!input) throw new Error("Maximum age input did not render");
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (!setValue) throw new Error("input value setter did not exist");
    input.focus();
    setValue.call(input, "123");
    await act(async () => input.dispatchEvent(new Event("input", { bubbles: true })));
    await act(async () => input.blur());
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    expect(journalRetentionSet).toHaveBeenCalledWith({ maxAgeMs: 123 });
    expect(container.querySelector('[role="alert"]')?.textContent ?? "").toContain("rejected");
    expect(input.getAttribute("value")).toBe("0");
    expect(input.value).toBe("0");
  });
});

describe("Settings providers catalog", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(journalUsage).mockResolvedValue({
      totalBytes: 0,
      sessionCount: 0,
      deletedByUser: 0,
      deletedByRetention: 0,
      unreclaimable: { bytesOver: 0, sessionsOver: 0, agedOut: 0 },
      limits: {
        snapshotEveryBytes: 65_536,
        sessionMaxBytes: 1,
        maxBytes: 1,
        maxSessions: 1,
        maxAgeMs: 0,
      },
      perSession: [],
    });
    vi.mocked(journalRetentionGet).mockResolvedValue({
      sessionMaxBytes: { value: 1, source: "default" },
      maxBytes: { value: 1, source: "default" },
      maxSessions: { value: 1, source: "default" },
      maxAgeMs: { value: 0, source: "default" },
    });
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it("renders PATH providers with ACP badge and unknown authentication", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "grok",
          executable: "C:\\\\npm\\\\grok.cmd",
          acpAvailable: true,
          authentication: "unknown",
        },
        {
          id: "claude",
          executable: "C:\\\\npm\\\\claude.cmd",
          acpAvailable: false,
          authentication: "unknown",
        },
      ],
      unreadableDirs: 0,
    });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    expect(container.textContent).toContain("grok");
    expect(container.textContent).toContain("C:\\\\npm\\\\grok.cmd");
    expect(container.textContent).toContain("ACP");
    expect(container.textContent).toContain("installed · authentication unknown");
    expect(container.textContent).toContain("claude");
    expect(container.querySelector('[role="switch"]')).toBeNull();
    expect(container.textContent).not.toContain("ready");
  });

  it("says when no agent CLI is on PATH", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({ providers: [], unreadableDirs: 0 });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    expect(container.textContent).toContain("No agent CLI found on PATH");
    expect(container.textContent).toContain("Install an agent CLI");
  });

  it("does not call an unreadable PATH scan an empty catalog", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({ providers: [], unreadableDirs: 3 });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    expect(container.textContent).toContain(
      "No agent CLI found, but 3 PATH directories could not be read",
    );
    expect(container.textContent).not.toContain("No agent CLI found on PATH");
  });

  it("notes unreadable PATH directories under a non-empty catalog", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "grok",
          executable: "C:\\\\npm\\\\grok.cmd",
          acpAvailable: true,
          authentication: "unknown",
        },
      ],
      unreadableDirs: 2,
    });
    root = createRoot(container);
    await act(async () => root.render(<SettingsSurface />));
    await act(async () => undefined);

    expect(container.textContent).toContain("grok");
    expect(container.textContent).toContain("2 PATH directories could not be read");
  });
});
