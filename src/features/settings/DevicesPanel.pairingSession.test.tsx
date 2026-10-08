// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/tauri")>();
  return {
    ...actual,
    devicesList: vi.fn(),
    pairingStart: vi.fn(),
    pairingComplete: vi.fn(),
    pairingConfirm: vi.fn(),
    peerRevoke: vi.fn(),
    peerSetCaps: vi.fn(),
  };
});

import { devicesList, pairingComplete, pairingStart } from "../../lib/tauri";
import type { DevicesReply, PairingCode, PeerRow, PendingPairing, SelfInfo } from "../../types/ipc";
import { DevicesPanel } from "./DevicesPanel";
import { resetPairingSession } from "./pairingSession";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// Pinned by beforeEach: the countdowns read the clock, so every expiry below is
// computed from this instant and never from the machine's own time.
const NOW = Date.now();

const SELF: SelfInfo = {
  deviceId: "1f0b7f3e-8b5a-4c2d-9e1f-0a1b2c3d4e5f",
  displayName: "Marcolenovo",
  publicKey: "cHVibGljLWtleS1vZi10aGlzLWRldmljZQ==",
  keyFingerprint: "0a1b2c3d4e5f60718293a4b5c6d7e8f9",
  addresses: ["100.102.128.70"],
  port: 47831,
  daemonVersion: "0.1.0",
  protocolVersion: 5,
  remote: { state: "enabled", reason: null },
};

const CODE: PairingCode = {
  code: "ABCD2345",
  expiresAt: NOW + 300_000,
  address: "100.102.128.70:47831",
};

const PENDING: PendingPairing = {
  deviceId: "3ac1f0de-4b5a-4c3d-8e9f-0a1b2c3d4e5f",
  displayName: "Marco's MacBook Pro",
  role: "daemon",
  keyFingerprint: "0123456789abcdef0123456789abcdef",
  address: "100.74.116.126:47831",
  expiresAt: NOW + 60_000,
};

// The far side's row once it confirms the pairing this device is waiting on.
const CONFIRMED_PEER: PeerRow = {
  deviceId: PENDING.deviceId,
  displayName: PENDING.displayName,
  role: "client",
  publicKey: "cGVlci1wdWJsaWMta2V5",
  keyFingerprint: PENDING.keyFingerprint,
  bindingKind: "tailnet",
  bindingNodeName: "macbook.tail80a42d.ts.net.",
  bindingLoginName: "user@example.com",
  address: PENDING.address,
  pairedAt: NOW,
  revokedAt: null,
  caps: ["view"],
  pairedByUser: "S-1-5-21-1",
  online: true,
};

function replyWith(overrides: Partial<DevicesReply> = {}): DevicesReply {
  return {
    selfInfo: SELF,
    peers: overrides.peers ?? [],
    pending: overrides.pending ?? [],
  };
}

describe("pairing session across a Settings page switch", () => {
  let container: HTMLDivElement;
  // Null between mounts: a page switch unmounts the panel and the test remounts
  // it on the same container, so the cleanup must find nothing to unmount.
  let root: Root | null = null;

  function buttonByText(text: string): HTMLButtonElement {
    const found = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => (button.textContent ?? "").trim() === text,
    );
    if (found === undefined) throw new Error(`button did not render: ${text}`);
    return found;
  }

  function addressInput(): HTMLInputElement {
    const field = container.querySelector<HTMLInputElement>('input[placeholder^="100.64"]');
    if (field === null) throw new Error("address field did not render");
    return field;
  }

  function codeInput(): HTMLInputElement {
    const field = container.querySelector<HTMLInputElement>('input[aria-label="pairing code"]');
    if (field === null) throw new Error("code field did not render");
    return field;
  }

  async function typeInto(field: HTMLInputElement, text: string): Promise<void> {
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("input value setter did not exist");
    await act(async () => {
      setValue.call(field, text);
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });
  }

  async function clickButton(text: string): Promise<void> {
    await act(async () => {
      buttonByText(text).click();
      await Promise.resolve();
    });
  }

  async function renderPanel(): Promise<void> {
    const mounted = createRoot(container);
    root = mounted;
    await act(async () => {
      mounted.render(<DevicesPanel />);
      await Promise.resolve();
    });
  }

  /** What a page switch does: the panel leaves the tree. */
  async function unmountPanel(): Promise<void> {
    const mounted = root;
    root = null;
    if (mounted !== null) await act(async () => mounted.unmount());
  }

  async function showCode(): Promise<void> {
    await renderPanel();
    await clickButton("Show a code");
  }

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW);
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(devicesList).mockResolvedValue(replyWith());
    vi.mocked(pairingStart).mockResolvedValue(CODE);
    vi.mocked(pairingComplete).mockResolvedValue({ type: "pairing_pending", peer: PENDING });
  });

  afterEach(async () => {
    await unmountPanel();
    resetPairingSession();
    container.remove();
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("keeps the shown code across a page switch and does not mint a second one", async () => {
    await showCode();
    expect(container.textContent).toContain("ABCD 2345");

    await unmountPanel();
    await renderPanel();

    expect(container.textContent).toContain("ABCD 2345");
    expect(container.textContent).toContain(
      "Type this on the other device at 100.102.128.70:47831",
    );
    expect(pairingStart).toHaveBeenCalledTimes(1);
  });

  it("keeps counting down from the stored expiry instant instead of restarting", async () => {
    await showCode();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });

    await unmountPanel();
    await renderPanel();

    expect(container.textContent).toContain("Expires in 4:00");
    expect(pairingStart).toHaveBeenCalledTimes(1);
  });

  it("drops the code once its countdown runs out while the panel was away", async () => {
    await showCode();

    await unmountPanel();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300_000);
    });
    await renderPanel();

    expect(container.textContent).not.toContain("ABCD 2345");
    expect(buttonByText("Enter a code").disabled).toBe(false);
  });

  it("forgets the code for good once the person cancels it", async () => {
    await showCode();
    await clickButton("Cancel");

    await unmountPanel();
    await renderPanel();

    expect(container.textContent).not.toContain("ABCD 2345");
  });

  it("keeps a code still being minted when the page switches, and does not ask again", async () => {
    let resolveStart: (code: PairingCode) => void = () => undefined;
    vi.mocked(pairingStart).mockReturnValueOnce(
      new Promise<PairingCode>((resolve) => {
        resolveStart = resolve;
      }),
    );
    await renderPanel();
    await clickButton("Show a code");

    await unmountPanel();
    await renderPanel();
    await clickButton("Asking…");
    await act(async () => {
      resolveStart(CODE);
      await Promise.resolve();
    });

    expect(pairingStart).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain("ABCD 2345");
  });

  it("clears the shown code when the other device spends it, and lists the parked pairing", async () => {
    await showCode();
    vi.mocked(devicesList).mockResolvedValue(replyWith({ pending: [PENDING] }));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });

    expect(container.textContent).not.toContain("ABCD 2345");
    expect(container.textContent).toContain("Waiting for your confirmation (1)");

    await unmountPanel();
    await renderPanel();
    expect(container.textContent).not.toContain("ABCD 2345");
  });

  it("does not take a parked pairing that was already listed before the code was shown as spent", async () => {
    vi.mocked(devicesList).mockResolvedValue(replyWith({ pending: [PENDING] }));
    await showCode();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });

    expect(container.textContent).toContain("ABCD 2345");
  });

  it("keeps the waiting card across a page switch, then clears it when the far side's row arrives", async () => {
    await renderPanel();
    await clickButton("Enter a code");
    await typeInto(addressInput(), "100.74.116.126:47831");
    await typeInto(codeInput(), "ABCD2345");
    await act(async () => {
      const form = container.querySelector("form");
      if (form === null) throw new Error("pairing form did not render");
      form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await Promise.resolve();
    });
    expect(container.textContent).toContain("Waiting for Marco's MacBook Pro to confirm");

    await unmountPanel();
    await renderPanel();
    expect(container.textContent).toContain("Waiting for Marco's MacBook Pro to confirm");
    expect(pairingComplete).toHaveBeenCalledTimes(1);

    vi.mocked(devicesList).mockResolvedValue(replyWith({ peers: [CONFIRMED_PEER] }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });

    expect(container.textContent).not.toContain("Waiting for Marco's MacBook Pro to confirm");
    expect(container.textContent).toContain("Paired with Marco's MacBook Pro (client).");
  });

  it("keeps the enter draft, address and code text, across a page switch", async () => {
    await renderPanel();
    await clickButton("Enter a code");
    await typeInto(addressInput(), "100.74.116.126:47831");
    await typeInto(codeInput(), "ABCD2345");

    await unmountPanel();
    await renderPanel();

    expect(addressInput().value).toBe("100.74.116.126:47831");
    expect(codeInput().value).toBe("ABCD2345");
    expect(pairingComplete).not.toHaveBeenCalled();
  });
});
