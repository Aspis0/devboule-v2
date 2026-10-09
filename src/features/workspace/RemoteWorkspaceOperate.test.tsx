// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  remoteHostClaim,
  remoteHostClose,
  remoteHostCreate,
  remoteHostInterrupt,
  remoteHostList,
  remoteHostPermissionRespond,
  remoteHostProviders,
  remoteHostResize,
  remoteHostSend,
  remoteHostStop,
  remoteSessionAttach,
  remoteSessionDetach,
} from "../../lib/tauri";
import type { ProviderInfo, RemoteRelayMessage, Session } from "../../types/ipc";
import { RemoteWorkspaceSurface } from "./RemoteWorkspaceSurface";
import { composerDrivers } from "./composerTestKit";

const mockEventHandlers: ((message: RemoteRelayMessage) => void)[] = [];

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  remoteHostList: vi.fn(),
  remoteSessionAttach: vi.fn(),
  remoteSessionDetach: vi.fn(),
  remoteHostCreate: vi.fn(),
  remoteHostSend: vi.fn(),
  remoteHostResize: vi.fn(),
  remoteHostClaim: vi.fn(),
  remoteHostInterrupt: vi.fn(),
  remoteHostPermissionRespond: vi.fn(),
  remoteHostClose: vi.fn(),
  remoteHostStop: vi.fn(),
  remoteHostProviders: vi.fn(),
  createRemoteEventChannel: vi.fn((onMessage: (message: RemoteRelayMessage) => void) => {
    mockEventHandlers.push(onMessage);
    return {} as never;
  }),
}));

const viewWrites: string[] = [];
let viewData: ((data: string) => void) | null = null;
vi.mock("../terminal/createTerminalView", () => ({
  createTerminalView: vi.fn((_host: HTMLElement, options: { onData: (data: string) => void }) => {
    viewData = options.onData;
    return {
      write: vi.fn((data: string) => {
        viewWrites.push(data);
      }),
      applySnapshot: vi.fn(),
      fit: vi.fn(() => true),
      dispose: vi.fn(),
      cols: vi.fn(() => 80),
      rows: vi.fn(() => 24),
    };
  }),
}));

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const AGENT: Session = {
  id: "session-one",
  workspaceId: "workspace-one",
  kind: "claude",
  title: "Agent one",
  state: { type: "live", generation: 1 },
  elapsedMs: null,
};

const TERMINAL: Session = {
  id: "terminal-one",
  workspaceId: "workspace-one",
  kind: "terminal",
  title: "Shell one",
  state: { type: "live", generation: 1 },
  elapsedMs: null,
};

const CLAUDE_PROVIDER: ProviderInfo = {
  id: "claude",
  executable: "claude",
  acpAvailable: false,
  authentication: "ok",
  protocol: "stream-json",
  origin: "user-binary",
  installed: true,
};

function acpProvider(id: string): ProviderInfo {
  return {
    id,
    executable: id,
    acpAvailable: true,
    authentication: "ok",
    protocol: "acp",
    origin: "user-binary",
    installed: true,
  };
}

describe("the remote operate surface", () => {
  let container: HTMLDivElement;
  let root: Root;

  async function flush(): Promise<void> {
    await act(async () => {
      for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();
    });
  }

  async function render(hostOnline = true): Promise<void> {
    await act(async () => {
      root.render(
        <RemoteWorkspaceSurface
          deviceId="device-one"
          workspaceId="workspace-one"
          hostOnline={hostOnline}
        />,
      );
    });
    await flush();
  }

  function emit(message: RemoteRelayMessage): void {
    for (const handler of mockEventHandlers) handler(message);
  }

  async function openSession(id: string): Promise<number> {
    const chip = [...container.querySelectorAll<HTMLButtonElement>("[role='tab']")].find((button) =>
      button.textContent?.includes(id === "session-one" ? "Agent one" : "Shell one"),
    );
    await act(async () => {
      chip?.click();
    });
    await flush();
    return vi.mocked(remoteSessionAttach).mock.calls.at(-1)?.[2] as number;
  }

  function lastCreateKey(): string {
    const call = vi.mocked(remoteHostCreate).mock.calls.at(-1);
    expect(call).toBeDefined();
    return (call?.[0] as { idempotencyKey: string }).idempotencyKey;
  }

  beforeEach(() => {
    mockEventHandlers.length = 0;
    viewWrites.length = 0;
    viewData = null;
    vi.mocked(remoteHostList).mockResolvedValue({ list: "sessions", rows: [AGENT] });
    vi.mocked(remoteSessionAttach).mockResolvedValue(undefined);
    vi.mocked(remoteSessionDetach).mockResolvedValue(undefined);
    vi.mocked(remoteHostCreate).mockImplementation(async (args) => ({
      ...AGENT,
      id: "session-new",
      kind: args.kind,
    }));
    vi.mocked(remoteHostSend).mockResolvedValue(true);
    vi.mocked(remoteHostResize).mockResolvedValue(undefined);
    vi.mocked(remoteHostClaim).mockResolvedValue(undefined);
    vi.mocked(remoteHostInterrupt).mockResolvedValue(undefined);
    vi.mocked(remoteHostPermissionRespond).mockResolvedValue(undefined);
    vi.mocked(remoteHostClose).mockResolvedValue(undefined);
    vi.mocked(remoteHostStop).mockResolvedValue(undefined);
    vi.mocked(remoteHostProviders).mockResolvedValue({
      providers: [CLAUDE_PROVIDER],
      unreadableDirs: 0,
    });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  it("creates an agent on the host with the host's provider and one retry identity", async () => {
    await render();
    const add = container.querySelector<HTMLButtonElement>("button[aria-label='New remote tab']");
    await act(async () => {
      add?.click();
    });
    await flush();
    const agentEntry = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
      (button) => button.textContent === "Agent",
    );
    await act(async () => {
      agentEntry?.click();
    });
    await flush();

    // One capable provider on the host goes straight through: no picker.
    expect(vi.mocked(remoteHostProviders)).toHaveBeenCalledWith("device-one");
    expect(vi.mocked(remoteHostCreate)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(remoteHostCreate)).toHaveBeenCalledWith({
      deviceId: "device-one",
      workspaceId: "workspace-one",
      kind: "claude",
      provider: null,
      idempotencyKey: expect.any(String),
    });
    // The created session opens under the host's workspace: the roster is
    // re-read and the new row is attached.
    expect(vi.mocked(remoteHostList).mock.calls.length).toBeGreaterThan(1);
    expect(remoteSessionAttach).toHaveBeenCalledWith(
      "device-one",
      "session-new",
      expect.any(Number),
      expect.anything(),
    );
  });

  it("offers the host's providers when several can chat", async () => {
    vi.mocked(remoteHostProviders).mockResolvedValue({
      providers: [CLAUDE_PROVIDER, acpProvider("codex-acp")],
      unreadableDirs: 0,
    });
    await render();
    const add = container.querySelector<HTMLButtonElement>("button[aria-label='New remote tab']");
    await act(async () => {
      add?.click();
    });
    await flush();
    const agentEntry = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
      (button) => button.textContent === "Agent",
    );
    await act(async () => {
      agentEntry?.click();
    });
    await flush();

    expect(vi.mocked(remoteHostCreate)).not.toHaveBeenCalled();
    const options = [...container.querySelectorAll<HTMLButtonElement>("[role='option']")].map(
      (button) => button.textContent,
    );
    expect(options).toEqual(["claude", "codex-acp"]);
    await act(async () => {
      container.querySelectorAll<HTMLButtonElement>("[role='option']")[1]?.click();
    });
    await flush();
    expect(vi.mocked(remoteHostCreate)).toHaveBeenCalledWith({
      deviceId: "device-one",
      workspaceId: "workspace-one",
      kind: "acp",
      provider: "codex-acp",
      idempotencyKey: expect.any(String),
    });
  });

  it("creates a terminal on the host with no provider", async () => {
    vi.mocked(remoteHostCreate).mockImplementation(async () => TERMINAL);
    await render();
    const add = container.querySelector<HTMLButtonElement>("button[aria-label='New remote tab']");
    await act(async () => {
      add?.click();
    });
    await flush();
    const terminalEntry = [
      ...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']"),
    ].find((button) => button.textContent === "Terminal");
    await act(async () => {
      terminalEntry?.click();
    });
    await flush();

    expect(vi.mocked(remoteHostCreate)).toHaveBeenCalledWith({
      deviceId: "device-one",
      workspaceId: "workspace-one",
      kind: "terminal",
      provider: null,
      idempotencyKey: expect.any(String),
    });
    expect(vi.mocked(remoteHostProviders)).not.toHaveBeenCalled();
  });

  it("keeps the retry identity across an explicit retry and never resends alone", async () => {
    vi.mocked(remoteHostCreate).mockRejectedValueOnce(new Error("The host stopped answering."));
    await render();
    const add = container.querySelector<HTMLButtonElement>("button[aria-label='New remote tab']");
    await act(async () => {
      add?.click();
    });
    await flush();
    const agentEntry = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
      (button) => button.textContent === "Agent",
    );
    await act(async () => {
      agentEntry?.click();
    });
    await flush();

    expect(vi.mocked(remoteHostCreate)).toHaveBeenCalledTimes(1);
    const key = lastCreateKey();
    // The failure stands with an explicit retry: nothing went out on its own.
    const retry = [...container.querySelectorAll<HTMLButtonElement>("button")].find(
      (button) => button.textContent === "Retry",
    );
    expect(retry).toBeDefined();
    await flush();
    expect(vi.mocked(remoteHostCreate)).toHaveBeenCalledTimes(1);

    await act(async () => {
      retry?.click();
    });
    await flush();
    expect(vi.mocked(remoteHostCreate)).toHaveBeenCalledTimes(2);
    expect(lastCreateKey()).toBe(key);
  });

  it("keeps the retry identity across a workspace switch", async () => {
    vi.mocked(remoteHostCreate).mockRejectedValueOnce(new Error("The host stopped answering."));
    await render();
    const add = container.querySelector<HTMLButtonElement>("button[aria-label='New remote tab']");
    await act(async () => {
      add?.click();
    });
    await flush();
    const agentEntry = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
      (button) => button.textContent === "Agent",
    );
    await act(async () => {
      agentEntry?.click();
    });
    await flush();
    const key = lastCreateKey();

    // Leaving the workspace remounts the surface; the failure and its key
    // come back with it instead of dying in component state.
    await act(async () => root.unmount());
    container.remove();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await render();

    const retry = [...container.querySelectorAll<HTMLButtonElement>("button")].find(
      (button) => button.textContent === "Retry",
    );
    expect(retry).toBeDefined();
    await act(async () => {
      retry?.click();
    });
    await flush();
    expect(vi.mocked(remoteHostCreate)).toHaveBeenCalledTimes(2);
    expect(lastCreateKey()).toBe(key);
  });

  it("sends composer text on the attached subscription", async () => {
    await render();
    const subscription = await openSession("session-one");
    const drivers = composerDrivers(container);
    await drivers.type("hello from A");
    await drivers.press("Enter");
    await flush();

    expect(vi.mocked(remoteHostSend)).toHaveBeenCalledTimes(1);
    const send = vi.mocked(remoteHostSend).mock.calls[0]?.[0];
    expect(send).toMatchObject({
      deviceId: "device-one",
      sessionId: "session-one",
      subscriptionId: subscription,
      text: "hello from A",
    });
    expect(typeof send?.idempotencyKey).toBe("string");
  });

  it("answers the host's permission card as the human, with no card of its own", async () => {
    await render();
    const subscription = await openSession("session-one");
    await act(async () => {
      emit({
        kind: "event",
        deviceId: "device-one",
        sessionId: "session-one",
        subscriptionId: subscription,
        envelope: {
          sessionId: "session-one",
          generation: 1,
          event: {
            type: "permission_request",
            toolCallId: "card-1",
            title: "Run tests",
            options: [
              { optionId: "allow", name: "Allow once", kind: "allow_once" },
              { optionId: "deny", name: "Deny", kind: "reject_once" },
            ],
          },
        },
      });
    });
    await flush();

    const allow = [...container.querySelectorAll<HTMLButtonElement>("button")].find(
      (button) => button.textContent === "Allow once",
    );
    expect(allow).toBeDefined();
    await act(async () => {
      allow?.click();
    });
    await flush();
    expect(vi.mocked(remoteHostPermissionRespond)).toHaveBeenCalledWith({
      deviceId: "device-one",
      sessionId: "session-one",
      subscriptionId: subscription,
      requestId: "card-1",
      outcome: "allow_once",
      idempotencyKey: expect.any(String),
    });

    // The host resolving the card clears it here.
    await act(async () => {
      emit({
        kind: "event",
        deviceId: "device-one",
        sessionId: "session-one",
        subscriptionId: subscription,
        envelope: {
          sessionId: "session-one",
          generation: 1,
          event: { type: "permission_resolved", toolCallId: "card-1" },
        },
      });
    });
    await flush();
    expect(
      [...container.querySelectorAll<HTMLButtonElement>("button")].find(
        (button) => button.textContent === "Allow once",
      ),
    ).toBeUndefined();
  });

  it("drives a terminal on the host: output, keys, claim and resize", async () => {
    vi.mocked(remoteHostList).mockResolvedValue({ list: "sessions", rows: [TERMINAL] });
    await render();
    const subscription = await openSession("terminal-one");
    await flush();

    // The claim goes out once the stream is up, then the fitted grid.
    expect(vi.mocked(remoteHostClaim)).toHaveBeenCalledWith(
      "device-one",
      "terminal-one",
      subscription,
    );
    expect(vi.mocked(remoteHostResize)).toHaveBeenCalledWith(
      "device-one",
      "terminal-one",
      subscription,
      80,
      24,
    );

    // Host output lands in the view, never in a transcript.
    await act(async () => {
      emit({
        kind: "event",
        deviceId: "device-one",
        sessionId: "terminal-one",
        subscriptionId: subscription,
        envelope: {
          sessionId: "terminal-one",
          generation: 1,
          event: { type: "output", seq: 1, data: "B says hi" },
        },
      });
    });
    await flush();
    expect(viewWrites).toContain("B says hi");

    // Typing travels as a send on the same subscription.
    await act(async () => {
      viewData?.("echo typed\r");
    });
    await flush();
    expect(vi.mocked(remoteHostSend)).toHaveBeenCalledWith({
      deviceId: "device-one",
      sessionId: "terminal-one",
      subscriptionId: subscription,
      text: "echo typed\r",
    });
  });

  it("closes a session on the host from its tab", async () => {
    await render();
    const close = container.querySelector<HTMLButtonElement>("button[aria-label^='Close']");
    expect(close).toBeDefined();
    await act(async () => {
      close?.click();
    });
    await flush();
    expect(vi.mocked(remoteHostClose)).toHaveBeenCalledWith("device-one", "session-one");
  });

  it("archives an attached terminal on the host", async () => {
    vi.mocked(remoteHostList).mockResolvedValue({ list: "sessions", rows: [TERMINAL] });
    await render();
    const subscription = await openSession("terminal-one");
    await flush();
    const archive = [...container.querySelectorAll<HTMLButtonElement>("button")].find(
      (button) => button.textContent === "Archive",
    );
    expect(archive).toBeDefined();
    await act(async () => {
      archive?.click();
    });
    await flush();
    expect(vi.mocked(remoteHostStop)).toHaveBeenCalledWith(
      "device-one",
      "terminal-one",
      subscription,
    );
  });
});
