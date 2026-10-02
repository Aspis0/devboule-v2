// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daemonStatus, devicesList } from "../../../lib/tauri";
import type { DaemonStatus, DevicesReply, PeerRow } from "../../../types/ipc";
import { HostSections } from "./HostSections";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  daemonStatus: vi.fn(),
  devicesList: vi.fn(),
}));

const CONNECTED: DaemonStatus = {
  state: "connected",
  pid: 42,
  instanceId: "daemon-test",
  protocolVersion: 21,
  clients: 1,
  capabilities: [],
  message: null,
};

function peer(overrides: Partial<PeerRow> = {}): PeerRow {
  return {
    deviceId: "dev-studio",
    displayName: "Studio",
    role: "daemon",
    publicKey: "k",
    keyFingerprint: "aaaa bbbb",
    bindingKind: "tailnet",
    bindingNodeName: "studio.tailnet",
    bindingLoginName: null,
    address: "100.64.0.9:47831",
    pairedAt: 1,
    revokedAt: null,
    caps: ["view"],
    pairedByUser: null,
    online: true,
    ...overrides,
  };
}

function reply(peers: readonly PeerRow[]): DevicesReply {
  return {
    selfInfo: {
      deviceId: "self",
      displayName: "This PC",
      publicKey: "k",
      keyFingerprint: "cccc dddd",
      addresses: ["100.64.0.1"],
      port: 47831,
      daemonVersion: "0.1.0",
      protocolVersion: 21,
      remote: { state: "enabled", reason: null },
    },
    peers: [...peers],
    pending: [],
  };
}

describe("what the host sections remember between runs", () => {
  let container: HTMLDivElement;
  let root: Root;

  /** The poll answers in microtasks and only fires when a test advances it. */
  async function settle(): Promise<void> {
    await act(async () => {
      for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();
    });
  }

  async function render(): Promise<void> {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <HostSections daemon={CONNECTED}>
          <p>local body</p>
        </HostSections>,
      );
    });
    await settle();
  }

  /** Unmount and mount again: a new tree reading the same store. */
  async function remount(): Promise<void> {
    await act(async () => root.unmount());
    await render();
  }

  async function pollAgain(): Promise<void> {
    await act(async () => {
      vi.advanceTimersByTime(2000);
    });
    await settle();
  }

  function remoteHead(): HTMLButtonElement {
    const head = container.querySelectorAll<HTMLElement>(
      ".sidebar-host-section .sidebar-host-head",
    )[1];
    if (!(head instanceof HTMLButtonElement)) {
      throw new Error("the remote host header did not render as a control");
    }
    return head;
  }

  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    vi.mocked(devicesList).mockResolvedValue(reply([peer()]));
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("keeps a fold across a rebuild of the surface", async () => {
    await render();

    await act(async () => remoteHead().click());
    expect(remoteHead().getAttribute("aria-expanded")).toBe("false");

    await remount();

    expect(remoteHead().getAttribute("aria-expanded")).toBe("false");
    expect(container.textContent).not.toContain("not available in this version");
  });

  it("puts a host that appears later at the end, whatever its name", async () => {
    vi.mocked(devicesList).mockResolvedValue(
      reply([peer({ deviceId: "dev-m", displayName: "Mika" })]),
    );
    await render();
    expect(remoteHead().textContent).toContain("Mika");

    vi.mocked(devicesList).mockResolvedValue(
      reply([
        peer({ deviceId: "dev-m", displayName: "Mika" }),
        peer({ deviceId: "dev-a", displayName: "alpha" }),
      ]),
    );
    await pollAgain();

    const heads = [...container.querySelectorAll<HTMLElement>(".sidebar-host-section")].map(
      (section) => section.querySelector(".sidebar-host-head")?.textContent ?? "",
    );
    expect(heads[1]).toContain("Mika");
    expect(heads[2]).toContain("alpha");
  });

  it("keeps a host where it was across a poll that left it out", async () => {
    const mika = peer({ deviceId: "dev-m", displayName: "Mika" });
    const zeta = peer({ deviceId: "dev-z", displayName: "Zeta" });
    const alpha = peer({ deviceId: "dev-a", displayName: "alpha" });
    function heads(): string[] {
      return [...container.querySelectorAll<HTMLElement>(".sidebar-host-section")].map(
        (section) => section.querySelector(".sidebar-host-head")?.textContent ?? "",
      );
    }

    vi.mocked(devicesList).mockResolvedValue(reply([zeta]));
    await render();

    vi.mocked(devicesList).mockResolvedValue(reply([zeta, mika]));
    await pollAgain();
    expect(heads()[1]).toContain("Zeta");
    expect(heads()[2]).toContain("Mika");

    // Mika goes away and a host that sorts before it is discovered.
    vi.mocked(devicesList).mockResolvedValue(reply([zeta, alpha]));
    await pollAgain();
    expect(heads()).toHaveLength(3);
    expect(heads()[1]).toContain("Zeta");
    expect(heads()[2]).toContain("alpha");

    vi.mocked(devicesList).mockResolvedValue(reply([zeta, alpha, mika]));
    await pollAgain();
    // alpha was discovered while Mika was away, so it took the place after
    // Mika's remembered slot — not Mika's.
    expect(heads()[1]).toContain("Zeta");
    expect(heads()[2]).toContain("Mika");
    expect(heads()[3]).toContain("alpha");
  });

  it("writes once when the answer changes, and not again for the same one", async () => {
    const setItem = vi.spyOn(window.localStorage, "setItem");
    await render();
    setItem.mockClear();

    await pollAgain();
    expect(setItem).not.toHaveBeenCalled();

    vi.mocked(devicesList).mockResolvedValue(
      reply([peer(), peer({ deviceId: "dev-m", displayName: "Mika" })]),
    );
    await pollAgain();
    expect(setItem).toHaveBeenCalledTimes(1);
  });

  it("keeps folding with a store that refuses the write", async () => {
    vi.spyOn(window.localStorage, "setItem").mockImplementation(() => {
      throw new Error("QuotaExceededError");
    });

    await render();

    await act(async () => remoteHead().click());

    expect(remoteHead().getAttribute("aria-expanded")).toBe("false");
    expect(container.textContent).not.toContain("not available in this version");
  });
});
