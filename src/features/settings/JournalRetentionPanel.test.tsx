// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/tauri")>();
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

vi.mock("../oracle/OraclePanel", () => ({
  OraclePanel: () => <div>Oracle mock</div>,
}));

import { journalRetentionGet, journalRetentionSet, journalUsage } from "../../lib/tauri";
import type { AgentProfilesDocument, JournalRetention } from "../../types/ipc";
import { JournalRetentionPanel } from "./JournalRetentionPanel";

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
    await act(async () => root.render(<JournalRetentionPanel />));
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
    await act(async () => root.render(<JournalRetentionPanel />));
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
    await act(async () => root.render(<JournalRetentionPanel />));
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
    await act(async () => root.render(<JournalRetentionPanel />));
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
    await act(async () => root.render(<JournalRetentionPanel />));
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
    expect(container.querySelector('[role="alert"]')?.textContent ?? "").toContain(
      "The agent daemon refused that request as invalid.",
    );
    expect(input.getAttribute("value")).toBe("0");
    expect(input.value).toBe("0");
  });
});
