// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { remoteHostList, remoteSessionAttach, remoteSessionDetach } from "../../lib/tauri";
import type { RemoteRelayMessage, Session } from "../../types/ipc";
import { RemoteWorkspaceSurface } from "./RemoteWorkspaceSurface";

const mockEventHandlers: ((message: RemoteRelayMessage) => void)[] = [];

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  remoteHostList: vi.fn(),
  remoteSessionAttach: vi.fn(),
  remoteSessionDetach: vi.fn(),
  createRemoteEventChannel: vi.fn((onMessage: (message: RemoteRelayMessage) => void) => {
    mockEventHandlers.push(onMessage);
    return {} as never;
  }),
}));

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const SESSION: Session = {
  id: "session-one",
  workspaceId: "workspace-one",
  kind: "claude",
  title: "Agent one",
  state: { type: "live", generation: 1 },
  elapsedMs: null,
};

describe("the remote workspace surface", () => {
  let container: HTMLDivElement;
  let root: Root;

  async function flush(): Promise<void> {
    await act(async () => {
      for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();
    });
  }

  async function render(hostOnline: boolean): Promise<void> {
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

  beforeEach(() => {
    mockEventHandlers.length = 0;
    vi.mocked(remoteHostList).mockResolvedValue({ list: "sessions", rows: [SESSION] });
    vi.mocked(remoteSessionAttach).mockResolvedValue(undefined);
    vi.mocked(remoteSessionDetach).mockResolvedValue(undefined);
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  it("lists the host's sessions and opens one with a live stream", async () => {
    await render(true);
    const chip = [...container.querySelectorAll<HTMLButtonElement>("[role='tab']")].find(
      (button) => button.textContent === "Agent one",
    );
    expect(chip).toBeDefined();

    await act(async () => {
      chip?.click();
    });
    await flush();
    expect(remoteSessionAttach).toHaveBeenCalledWith(
      "device-one",
      "session-one",
      1,
      expect.anything(),
    );

    await act(async () => {
      emit({
        kind: "event",
        deviceId: "device-one",
        sessionId: "session-one",
        subscriptionId: 1,
        envelope: {
          sessionId: "session-one",
          generation: 1,
          event: { type: "output", seq: 1, data: "hello remote" },
        },
      });
    });
    await flush();
    expect(container.textContent).toContain("hello remote");
  });

  it("replays a gapped stream on a fresh subscription", async () => {
    await render(true);
    const chip = [...container.querySelectorAll<HTMLButtonElement>("[role='tab']")][0];
    await act(async () => {
      chip?.click();
    });
    await flush();
    await act(async () => {
      emit({
        kind: "event",
        deviceId: "device-one",
        sessionId: "session-one",
        subscriptionId: 1,
        envelope: {
          sessionId: "session-one",
          generation: 1,
          event: { type: "output", seq: 1, data: "before the gap" },
        },
      });
    });
    await flush();
    expect(container.textContent).toContain("before the gap");

    await act(async () => {
      emit({
        kind: "gap",
        deviceId: "device-one",
        sessionId: "session-one",
        subscriptionId: 1,
      });
    });
    await flush();
    expect(remoteSessionDetach).toHaveBeenCalledWith("device-one", "session-one", 1);
    expect(remoteSessionAttach).toHaveBeenCalledTimes(2);
    expect(vi.mocked(remoteSessionAttach).mock.calls.at(-1)?.[2]).toBe(2);
    expect(container.textContent).not.toContain("before the gap");

    await act(async () => {
      emit({
        kind: "event",
        deviceId: "device-one",
        sessionId: "session-one",
        subscriptionId: 2,
        envelope: {
          sessionId: "session-one",
          generation: 1,
          event: { type: "output", seq: 2, data: "replayed after the gap" },
        },
      });
    });
    await flush();
    expect(container.textContent).toContain("replayed after the gap");
  });

  it("shows one short offline state and reattaches when the host returns", async () => {
    await render(true);
    const chip = [...container.querySelectorAll<HTMLButtonElement>("[role='tab']")][0];
    await act(async () => {
      chip?.click();
    });
    await flush();
    expect(remoteSessionAttach).toHaveBeenCalledTimes(1);

    await render(false);
    expect(container.textContent).toContain("offline");

    await render(true);
    expect(remoteSessionAttach).toHaveBeenCalledTimes(2);
    const lastCall = vi.mocked(remoteSessionAttach).mock.calls.at(-1);
    expect(lastCall?.[2]).toBe(2);

    await act(async () => {
      emit({
        kind: "event",
        deviceId: "device-one",
        sessionId: "session-one",
        subscriptionId: 2,
        envelope: {
          sessionId: "session-one",
          generation: 1,
          event: { type: "output", seq: 2, data: "back again" },
        },
      });
    });
    await flush();
    expect(container.textContent).toContain("back again");
  });
});
