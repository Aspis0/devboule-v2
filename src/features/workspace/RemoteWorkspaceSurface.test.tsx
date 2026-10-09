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
      expect.any(Number),
      expect.anything(),
    );
    // Subscription ids are process-wide now, so the test reads the one the
    // surface actually used instead of assuming 1.
    const opened = vi.mocked(remoteSessionAttach).mock.calls.at(-1)?.[2] as number;

    await act(async () => {
      emit({
        kind: "event",
        deviceId: "device-one",
        sessionId: "session-one",
        subscriptionId: opened,
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

  it("keeps the roster live: a status word and a periodic re-read", async () => {
    vi.useFakeTimers();
    await render(true);
    expect(container.textContent).not.toContain("waiting");

    // The host's next roster read reports the session waiting for approval;
    // the interval picks it up without any local action.
    vi.mocked(remoteHostList).mockResolvedValue({
      list: "sessions",
      rows: [
        {
          ...SESSION,
          attention: { reason: "permission", atMs: 1 },
        },
      ],
    });
    await act(async () => {
      vi.advanceTimersByTime(5100);
    });
    await flush();
    expect(container.textContent).toContain("waiting");
    vi.useRealTimers();
  });

  it("pauses the roster poll while the window is hidden", async () => {
    vi.useFakeTimers();
    await render(true);
    const afterMount = vi.mocked(remoteHostList).mock.calls.length;

    Object.defineProperty(document, "hidden", { configurable: true, value: true });
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await act(async () => {
      vi.advanceTimersByTime(5100);
    });
    await flush();
    expect(vi.mocked(remoteHostList).mock.calls.length).toBe(afterMount);

    Object.defineProperty(document, "hidden", { configurable: true, value: false });
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await flush();
    expect(vi.mocked(remoteHostList).mock.calls.length).toBe(afterMount + 1);

    await act(async () => {
      vi.advanceTimersByTime(5100);
    });
    await flush();
    expect(vi.mocked(remoteHostList).mock.calls.length).toBeGreaterThan(afterMount + 1);
    vi.useRealTimers();
  });

  it("coalesces event-triggered roster reloads", async () => {
    vi.useFakeTimers();
    await render(true);
    const chip = [...container.querySelectorAll<HTMLButtonElement>("[role='tab']")][0];
    await act(async () => {
      chip?.click();
    });
    await flush();
    const subscription = vi.mocked(remoteSessionAttach).mock.calls.at(-1)?.[2] as number;
    const before = vi.mocked(remoteHostList).mock.calls.length;

    await act(async () => {
      for (let index = 0; index < 5; index += 1) {
        emit({
          kind: "event",
          deviceId: "device-one",
          sessionId: "session-one",
          subscriptionId: subscription,
          envelope: {
            sessionId: "session-one",
            generation: 1,
            event: { type: "detached" },
          },
        });
      }
    });
    await flush();
    expect(vi.mocked(remoteHostList).mock.calls.length - before).toBeLessThanOrEqual(1);

    await act(async () => {
      vi.advanceTimersByTime(1100);
    });
    await flush();
    expect(vi.mocked(remoteHostList).mock.calls.length).toBeGreaterThan(before + 1);
    vi.useRealTimers();
  });

  it("clears a selection the host no longer lists", async () => {
    vi.useFakeTimers();
    await render(true);
    const chip = [...container.querySelectorAll<HTMLButtonElement>("[role='tab']")][0];
    await act(async () => {
      chip?.click();
    });
    await flush();
    expect(remoteSessionAttach).toHaveBeenCalledTimes(1);

    vi.mocked(remoteHostList).mockResolvedValue({ list: "sessions", rows: [] });
    await act(async () => {
      vi.advanceTimersByTime(5100);
    });
    await flush();
    vi.useRealTimers();
    expect(container.querySelectorAll("[role='tab']")).toHaveLength(0);
    // The vanished session cannot stay selected: its stream is given back.
    expect(remoteSessionDetach).toHaveBeenCalled();
  });

  it("serializes a fast tab switch with the last selection winning", async () => {
    vi.mocked(remoteHostList).mockResolvedValue({
      list: "sessions",
      rows: [SESSION, { ...SESSION, id: "session-two", title: "Agent two" }],
    });
    await render(true);
    const chips = [...container.querySelectorAll<HTMLButtonElement>("[role='tab']")];
    await act(async () => {
      chips[0]?.click();
      chips[1]?.click();
    });
    await flush();

    const calls = vi.mocked(remoteSessionAttach).mock.calls;
    expect(calls.length).toBeGreaterThanOrEqual(1);
    expect(calls.at(-1)?.[1]).toBe("session-two");
  });

  it("bounds the transcript and batches its appends", async () => {
    await render(true);
    const chip = [...container.querySelectorAll<HTMLButtonElement>("[role='tab']")][0];
    await act(async () => {
      chip?.click();
    });
    await flush();
    const subscription = vi.mocked(remoteSessionAttach).mock.calls.at(-1)?.[2] as number;
    await act(async () => {
      for (let index = 0; index < 3000; index += 1) {
        emit({
          kind: "event",
          deviceId: "device-one",
          sessionId: "session-one",
          subscriptionId: subscription,
          envelope: {
            sessionId: "session-one",
            generation: 1,
            event: { type: "output", seq: index, data: `line ${index}` },
          },
        });
      }
    });
    await flush();
    const rendered = container.querySelectorAll(".workspace-remote-transcript p");
    expect(rendered.length).toBeLessThanOrEqual(2000);
    expect(rendered.length).toBeGreaterThan(0);
    expect(container.textContent).toContain("line 2999");
  });

  it("replays a gapped stream on a fresh subscription", async () => {
    await render(true);
    const chip = [...container.querySelectorAll<HTMLButtonElement>("[role='tab']")][0];
    await act(async () => {
      chip?.click();
    });
    await flush();
    const first = vi.mocked(remoteSessionAttach).mock.calls.at(-1)?.[2] as number;
    await act(async () => {
      emit({
        kind: "event",
        deviceId: "device-one",
        sessionId: "session-one",
        subscriptionId: first,
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
        subscriptionId: first,
      });
    });
    await flush();
    expect(remoteSessionDetach).toHaveBeenCalledWith("device-one", "session-one", first);
    expect(remoteSessionAttach).toHaveBeenCalledTimes(2);
    const second = vi.mocked(remoteSessionAttach).mock.calls.at(-1)?.[2] as number;
    expect(second).not.toBe(first);
    expect(container.textContent).not.toContain("before the gap");

    await act(async () => {
      emit({
        kind: "event",
        deviceId: "device-one",
        sessionId: "session-one",
        subscriptionId: second,
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
    const first = vi.mocked(remoteSessionAttach).mock.calls[0][2];
    const second = vi.mocked(remoteSessionAttach).mock.calls[1][2];
    expect(second).not.toBe(first);

    await act(async () => {
      emit({
        kind: "event",
        deviceId: "device-one",
        sessionId: "session-one",
        subscriptionId: second,
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
