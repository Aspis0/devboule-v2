// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

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

import {
  devicesList,
  pairingComplete,
  pairingConfirm,
  pairingStart,
  peerRevoke,
  peerSetCaps,
} from "../../lib/tauri";
import type {
  DevicesReply,
  PairingCode,
  PeerRow,
  PendingPairing,
  RemoteState,
  SelfInfo,
} from "../../types/ipc";
import {
  DevicesPanel,
  formatDuration,
  groupFingerprint,
  relativeTime,
  remoteLabel,
  sanitizePairingCode,
} from "./DevicesPanel";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const FINGERPRINT = "0a1b2c3d4e5f60718293a4b5c6d7e8f9";
// Relative to the clock this test runs on: the panel formats times against
// `Date.now()`, so a hardcoded epoch would make every "paired … ago" wrong.
const NOW = Date.now();

const SELF: SelfInfo = {
  deviceId: "1f0b7f3e-8b5a-4c2d-9e1f-0a1b2c3d4e5f",
  displayName: "Marcolenovo",
  publicKey: "cHVibGljLWtleS1vZi10aGlzLWRldmljZQ==",
  keyFingerprint: FINGERPRINT,
  addresses: ["100.102.128.70"],
  port: 47831,
  daemonVersion: "0.1.0",
  protocolVersion: 5,
  remote: { state: "enabled", reason: null },
};

const CLIENT_PEER: PeerRow = {
  deviceId: "9f6b0f2e-6f1c-4a1e-9c62-1e2f7d59a9c3",
  displayName: "Xiaomi 14",
  role: "client",
  publicKey: "cGVlci1wdWJsaWMta2V5",
  keyFingerprint: "f9e8d7c6b5a4938271605f4e3d2c1b0a",
  bindingKind: "tailnet",
  bindingNodeName: "xiaomi-14.tail80a42d.ts.net.",
  bindingLoginName: "user@example.com",
  address: "100.74.116.126:47831",
  pairedAt: NOW - 3_600_000,
  revokedAt: null,
  caps: ["view"],
  pairedByUser: "S-1-5-21-1",
  online: true,
};

const REVOKED_PEER: PeerRow = {
  ...CLIENT_PEER,
  deviceId: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
  displayName: "TABLET-V477JRIG",
  role: "daemon",
  caps: [],
  online: false,
  revokedAt: NOW - 7_200_000,
};

const PENDING: PendingPairing = {
  deviceId: "3ac1f0de-4b5a-4c3d-8e9f-0a1b2c3d4e5f",
  displayName: "Marco's MacBook Pro",
  role: "daemon",
  keyFingerprint: "0123456789abcdef0123456789abcdef",
  address: "100.74.116.126:47831",
  expiresAt: NOW + 60_000,
};

// A second card, for the decline path: the daemon allows two parked pairings at
// once, so dropping one must leave the other on screen.
const SECOND_PENDING: PendingPairing = {
  ...PENDING,
  deviceId: "bbbbbbbb-cccc-dddd-eeee-ffffffffffff",
  displayName: "TABLET-V477JRIG",
  role: "client",
};

const CODE: PairingCode = {
  code: "ABCD2345",
  expiresAt: NOW + 300_000,
  address: "100.102.128.70:47831",
};

function replyWith(overrides: Partial<DevicesReply> = {}): DevicesReply {
  return {
    selfInfo: { ...SELF, ...overrides.selfInfo },
    peers: overrides.peers ?? [],
    pending: overrides.pending ?? [],
  };
}

/**
 * A `devicesList` stand-in that answers once and then never settles.
 *
 * The decline tests have to prove the panel's own state change, so no later
 * poll may hand it a fresh list. One wrinkle this encodes: when the first reply
 * carries a pending pairing, the poll effect re-runs immediately (the cadence
 * flips from 2 s to 1 s), so the second call comes from the mount, not from the
 * click.
 */
function hangingAfterFirstReply(pending: readonly PendingPairing[]) {
  let calls = 0;
  return () => {
    calls += 1;
    if (calls === 1) return Promise.resolve(replyWith({ pending: [...pending] }));
    return new Promise<DevicesReply>(() => undefined);
  };
}

describe("devices panel", () => {
  let container: HTMLDivElement;
  // Null between mounts: the remote-state test mounts several roots of its own,
  // so the shared cleanup must be able to find nothing to unmount.
  let root: Root | null = null;

  function buttonByText(text: string): HTMLButtonElement {
    const found = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
      (button) => (button.textContent ?? "").trim() === text,
    );
    if (found === undefined) throw new Error(`button did not render: ${text}`);
    return found;
  }

  function codeInput(): HTMLInputElement {
    const field = container.querySelector<HTMLInputElement>('input[aria-label="pairing code"]');
    if (field === null) throw new Error("code field did not render");
    return field;
  }

  function checkboxByLabel(label: string): HTMLInputElement {
    const found = Array.from(container.querySelectorAll<HTMLLabelElement>("label")).find((node) =>
      (node.textContent ?? "").startsWith(label),
    );
    if (found === undefined) throw new Error(`checkbox did not render: ${label}`);
    const field = found.querySelector<HTMLInputElement>('input[type="checkbox"]');
    if (field === null) throw new Error(`checkbox did not render: ${label}`);
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

  async function renderPanel(): Promise<void> {
    const mounted = createRoot(container);
    root = mounted;
    await act(async () => {
      mounted.render(<DevicesPanel />);
      await Promise.resolve();
    });
  }

  async function unmountPanel(): Promise<void> {
    const mounted = root;
    root = null;
    if (mounted !== null) await act(async () => mounted.unmount());
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(devicesList).mockResolvedValue(replyWith());
    vi.mocked(pairingStart).mockResolvedValue(CODE);
    vi.mocked(pairingComplete).mockResolvedValue({ type: "pairing_done", peer: CLIENT_PEER });
    vi.mocked(pairingConfirm).mockResolvedValue(CLIENT_PEER);
    vi.mocked(peerRevoke).mockResolvedValue({ ...CLIENT_PEER, revokedAt: NOW });
    vi.mocked(peerSetCaps).mockResolvedValue({ ...CLIENT_PEER, caps: ["view", "send"] });
  });

  afterEach(async () => {
    await unmountPanel();
    container.remove();
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("renders the four sections from the daemon's reply", async () => {
    vi.mocked(devicesList).mockResolvedValue(
      replyWith({ peers: [CLIENT_PEER, REVOKED_PEER], pending: [PENDING] }),
    );
    await renderPanel();

    const text = container.textContent ?? "";
    expect(text).toContain("This device");
    expect(text).toContain("Marcolenovo");
    // The fingerprint is grouped the way a person reads it aloud.
    expect(text).toContain(groupFingerprint(FINGERPRINT));
    expect(text).toContain("100.102.128.70 · port 47831");
    expect(text).toContain("Reachable on the tailnet");
    expect(text).toContain("Pair a device");
    expect(text).toContain("Waiting for your confirmation (1)");
    expect(text).toContain("Marco's MacBook Pro");
    expect(text).toContain("0123 4567 89ab cdef 0123 4567 89ab cdef");
    expect(text).toContain("Paired devices (1)");
    expect(text).toContain("Xiaomi 14");
    expect(text).toContain("Revoked (1)");
    expect(text).toContain("TABLET-V477JRIG");
    // Paired rows carry what the daemon sent, never a joined path or a guess.
    expect(text).toContain("100.74.116.126:47831 · xiaomi-14.tail80a42d.ts.net.");
    expect(text).toContain("paired 1 h ago");
  });

  it("starts polling on mount", async () => {
    vi.useFakeTimers();
    await renderPanel();

    expect(devicesList).toHaveBeenCalledTimes(1);
    // The idle cadence is 2 s.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_999);
    });
    expect(devicesList).toHaveBeenCalledTimes(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(devicesList).toHaveBeenCalledTimes(2);
  });

  it("polls again at once when the first reply turns the fast cadence on", async () => {
    // Documented behaviour, not an accident of the test: the cadence is an
    // effect dependency, so a first reply that carries a pending pairing flips
    // it and restarts the loop immediately. The panel is then watching
    // something the user has to answer, and it says so by asking again now.
    vi.useFakeTimers();
    vi.mocked(devicesList).mockResolvedValue(replyWith({ pending: [PENDING] }));
    await renderPanel();

    expect(container.textContent).toContain("Waiting for your confirmation (1)");
    expect(devicesList).toHaveBeenCalledTimes(2);
  });

  it("drops to a 1 s cadence while a code is on screen", async () => {
    vi.useFakeTimers();
    await renderPanel();

    await act(async () => {
      buttonByText("Show a code").click();
      await Promise.resolve();
    });
    expect(container.textContent).toContain("ABCD 2345");
    const afterShow = vi.mocked(devicesList).mock.calls.length;

    await act(async () => {
      await vi.advanceTimersByTimeAsync(999);
    });
    expect(vi.mocked(devicesList).mock.calls.length).toBe(afterShow);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(vi.mocked(devicesList).mock.calls.length).toBe(afterShow + 1);
  });

  it("stops polling on unmount and writes no state afterwards", async () => {
    vi.useFakeTimers();
    const consoleError = vi.spyOn(console, "error");
    let resolveLate: ((reply: DevicesReply) => void) | undefined;
    vi.mocked(devicesList)
      .mockResolvedValueOnce(replyWith())
      .mockImplementationOnce(
        () =>
          new Promise<DevicesReply>((resolve) => {
            resolveLate = resolve;
          }),
      );

    await renderPanel();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_000);
    });
    expect(devicesList).toHaveBeenCalledTimes(2);

    await act(async () => root?.unmount());
    await act(async () => {
      resolveLate?.(replyWith({ selfInfo: { ...SELF, displayName: "late reply" } }));
      await vi.advanceTimersByTimeAsync(10_000);
    });

    // The in-flight reply landed after unmount and scheduled nothing: no third
    // request, no state write, and React never complained about one.
    expect(devicesList).toHaveBeenCalledTimes(2);
    expect(consoleError).not.toHaveBeenCalled();
    expect(container.textContent ?? "").not.toContain("late reply");
    consoleError.mockRestore();
  });

  it("renders the three remote states, and the daemon's reason verbatim for disabled", async () => {
    const cases: Array<{ remote: RemoteState; expected: string }> = [
      { remote: { state: "enabled", reason: null }, expected: "Reachable on the tailnet" },
      {
        remote: { state: "disabled", reason: "Tailscale is not running" },
        expected: "Remote off · Tailscale is not running",
      },
      {
        remote: { state: "key_missing", reason: "no entry in the credential store" },
        expected: "Key missing · re-pair required",
      },
    ];
    for (const probe of cases) {
      vi.mocked(devicesList).mockResolvedValue(
        replyWith({ selfInfo: { ...SELF, remote: probe.remote } }),
      );
      await renderPanel();
      expect(container.textContent).toContain(probe.expected);
      await unmountPanel();
    }
  });

  it("shows a pairing code with its address and countdown, and Cancel just drops it", async () => {
    vi.useFakeTimers();
    await renderPanel();

    await act(async () => {
      buttonByText("Show a code").click();
      await Promise.resolve();
    });
    expect(pairingStart).toHaveBeenCalledWith("client");
    expect(container.textContent).toContain("ABCD 2345");
    expect(container.textContent).toContain(
      "Type this on the other device at 100.102.128.70:47831",
    );
    expect(container.textContent).toContain("Expires in 5:00");

    await act(async () => {
      buttonByText("Cancel").click();
      await Promise.resolve();
    });
    expect(container.textContent).not.toContain("ABCD 2345");
    // No cancel message exists in the protocol: the code is simply let go and
    // the daemon expires it.
    expect(vi.mocked(devicesList).mock.calls.length).toBeGreaterThan(0);
  });

  it("drops the code when its countdown runs out", async () => {
    vi.useFakeTimers();
    vi.mocked(pairingStart).mockResolvedValue({
      code: "ABCD2345",
      expiresAt: Date.now() + 3_000,
      address: "100.102.128.70:47831",
    });
    await renderPanel();

    await act(async () => {
      buttonByText("Show a code").click();
      await Promise.resolve();
    });
    expect(container.textContent).toContain("ABCD 2345");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3_000);
    });

    // The code expires on its own; no cancel message is sent anywhere.
    expect(container.textContent).not.toContain("ABCD 2345");
    expect(buttonByText("Enter a code").disabled).toBe(false);
  });

  it("shows a daemon pairing error verbatim inside the card", async () => {
    vi.mocked(pairingStart).mockRejectedValue({
      code: "invalid_request",
      message: "pairing is busy",
    });
    await renderPanel();

    await act(async () => {
      buttonByText("Show a code").click();
      await Promise.resolve();
    });

    const alert = container.querySelector('[role="alert"]');
    if (alert === null) throw new Error("pairing error did not render");
    expect(alert.textContent).toBe("pairing is busy");
  });

  it("keeps the two pairing actions mutually exclusive", async () => {
    vi.useFakeTimers();
    await renderPanel();

    await act(async () => {
      buttonByText("Enter a code").click();
      await Promise.resolve();
    });
    expect(buttonByText("Show a code").disabled).toBe(true);

    await act(async () => {
      buttonByText("Enter a code").click();
      await Promise.resolve();
    });

    await act(async () => {
      buttonByText("Show a code").click();
      await Promise.resolve();
    });
    expect(buttonByText("Enter a code").disabled).toBe(true);
    await act(async () => {
      buttonByText("Cancel").click();
      await Promise.resolve();
    });
    expect(buttonByText("Enter a code").disabled).toBe(false);
  });

  it("filters the typed code to the pairing alphabet and uppercases it", async () => {
    vi.mocked(devicesList).mockResolvedValue(replyWith());
    await renderPanel();

    await act(async () => {
      buttonByText("Enter a code").click();
      await Promise.resolve();
    });
    await typeInto(codeInput(), "ab01ioIZ23");

    expect(codeInput().value).toBe("ABZ23");
    expect(codeInput().value).not.toMatch(/[01IO]/);
  });

  it("strips spaces from a pasted code", async () => {
    await renderPanel();

    await act(async () => {
      buttonByText("Enter a code").click();
      await Promise.resolve();
    });
    await typeInto(codeInput(), "abcd 2345");

    expect(codeInput().value).toBe("ABCD2345");
  });

  it("reports a parked pairing while the far side confirms", async () => {
    vi.mocked(pairingComplete).mockResolvedValue({ type: "pairing_pending", peer: PENDING });
    await renderPanel();

    await act(async () => {
      buttonByText("Enter a code").click();
      await Promise.resolve();
    });
    await typeInto(codeInput(), "ABCD2345");
    await act(async () => {
      const form = container.querySelector("form");
      if (form === null) throw new Error("pairing form did not render");
      form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await Promise.resolve();
    });

    expect(pairingComplete).toHaveBeenCalledWith("", "ABCD2345", "client");
    expect(container.textContent).toContain("Waiting for Marco's MacBook Pro to confirm");
  });

  it("shows the paired row when the far side accepted", async () => {
    await renderPanel();

    await act(async () => {
      buttonByText("Enter a code").click();
      await Promise.resolve();
    });
    await typeInto(codeInput(), "ABCD2345");
    await act(async () => {
      const form = container.querySelector("form");
      if (form === null) throw new Error("pairing form did not render");
      form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await Promise.resolve();
    });

    expect(container.textContent).toContain("Paired with Xiaomi 14 (client).");
    // The panel re-polls immediately instead of waiting for the next tick.
    expect(vi.mocked(devicesList).mock.calls.length).toBeGreaterThan(1);
  });

  it("sends the typed address and role with the code", async () => {
    await renderPanel();

    await act(async () => {
      buttonByText("Enter a code").click();
      await Promise.resolve();
    });
    const address = container.querySelector<HTMLInputElement>('input[placeholder^="100.64"]');
    if (address === null) throw new Error("address field did not render");
    await typeInto(address, "100.74.116.126:47831");
    await typeInto(codeInput(), "abcd 2345");
    const daemonRadio = container.querySelector<HTMLInputElement>(
      'input[name="pairing-enter-role"][value="daemon"]',
    );
    if (daemonRadio === null) throw new Error("role choice did not render");
    await act(async () => {
      daemonRadio.click();
      await Promise.resolve();
    });
    await act(async () => {
      const form = container.querySelector("form");
      if (form === null) throw new Error("pairing form did not render");
      form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      await Promise.resolve();
    });

    expect(pairingComplete).toHaveBeenCalledWith("100.74.116.126:47831", "ABCD2345", "daemon");
  });

  it("answers a pending confirmation and re-polls", async () => {
    vi.mocked(devicesList).mockResolvedValue(replyWith({ pending: [PENDING] }));
    await renderPanel();

    expect(container.textContent).toContain(
      "Compare the fingerprint below with the one shown on that device",
    );
    await act(async () => {
      buttonByText("Confirm pairing").click();
      await Promise.resolve();
    });

    expect(pairingConfirm).toHaveBeenCalledWith(PENDING.deviceId, true);
    expect(vi.mocked(devicesList).mock.calls.length).toBeGreaterThan(1);
  });

  it("drops the card on a decline, which is a success and not an error", async () => {
    // The daemon answers a decline with `pairing_declined`, so the wrapper
    // resolves `null` instead of rejecting. Every poll after the first one is
    // left hanging on purpose: taking the card off screen is the panel's own
    // state change, and nothing on screen may suggest the decline failed.
    vi.mocked(devicesList).mockImplementation(hangingAfterFirstReply([PENDING]));
    vi.mocked(pairingConfirm).mockResolvedValue(null);
    await renderPanel();

    expect(container.textContent).toContain("Waiting for your confirmation (1)");
    await act(async () => {
      buttonByText("Decline").click();
      await Promise.resolve();
    });

    expect(pairingConfirm).toHaveBeenCalledWith(PENDING.deviceId, false);
    expect(container.textContent).not.toContain("Waiting for your confirmation");
    expect(container.textContent).not.toContain("Marco's MacBook Pro");
    expect(container.querySelector('[role="alert"]')).toBeNull();
    // The panel asked the daemon again: taking the card off screen is its own
    // belief, and only the next reply confirms it.
    expect(vi.mocked(devicesList).mock.calls.length).toBeGreaterThan(1);
  });

  it("drops only the declined card when two are waiting", async () => {
    vi.mocked(devicesList).mockImplementation(hangingAfterFirstReply([PENDING, SECOND_PENDING]));
    vi.mocked(pairingConfirm).mockResolvedValue(null);
    await renderPanel();

    expect(container.textContent).toContain("Waiting for your confirmation (2)");
    const declineButtons = Array.from(
      container.querySelectorAll<HTMLButtonElement>("button"),
    ).filter((button) => (button.textContent ?? "").trim() === "Decline");
    expect(declineButtons).toHaveLength(2);
    await act(async () => {
      // The second card, so the survivor is the first one.
      declineButtons[1]?.click();
      await Promise.resolve();
    });

    expect(pairingConfirm).toHaveBeenCalledWith(SECOND_PENDING.deviceId, false);
    expect(container.textContent).not.toContain("TABLET-V477JRIG");
    expect(container.textContent).toContain("Marco's MacBook Pro");
    expect(container.textContent).toContain("Waiting for your confirmation (1)");
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("shows a failed confirmation verbatim on the card it belongs to", async () => {
    vi.mocked(devicesList).mockResolvedValue(replyWith({ pending: [PENDING] }));
    vi.mocked(pairingConfirm).mockRejectedValue({
      code: "invalid_request",
      message: "no pairing is waiting for that device",
    });
    await renderPanel();

    await act(async () => {
      buttonByText("Confirm pairing").click();
      await Promise.resolve();
    });

    const alert = container.querySelector('[role="alert"]');
    if (alert === null) throw new Error("confirmation error did not render");
    expect(alert.textContent).toBe("no pairing is waiting for that device");
  });

  it("sends the full capability array on a toggle and keeps the optimistic flip", async () => {
    vi.mocked(devicesList).mockResolvedValue(replyWith({ peers: [CLIENT_PEER] }));
    await renderPanel();

    const send = checkboxByLabel("send");
    expect(send.checked).toBe(false);
    await act(async () => {
      send.click();
      await Promise.resolve();
    });

    expect(peerSetCaps).toHaveBeenCalledWith(CLIENT_PEER.deviceId, ["view", "send"]);
    // The daemon stores what it is given, so the array carries `view` too.
    expect(checkboxByLabel("send").checked).toBe(true);
  });

  it("keeps view checked and disabled, and never sends it away", async () => {
    vi.mocked(devicesList).mockResolvedValue(replyWith({ peers: [CLIENT_PEER] }));
    await renderPanel();

    const view = checkboxByLabel("view");
    expect(view.checked).toBe(true);
    expect(view.disabled).toBe(true);
    expect(container.textContent).toContain("Client peers can always view their own sessions");
  });

  it("reverts a capability toggle when the daemon refuses it", async () => {
    vi.mocked(devicesList).mockResolvedValue(replyWith({ peers: [CLIENT_PEER] }));
    vi.mocked(peerSetCaps).mockRejectedValue({
      code: "invalid_request",
      message: "no such peer",
    });
    await renderPanel();

    await act(async () => {
      checkboxByLabel("send").click();
      await Promise.resolve();
    });

    expect(checkboxByLabel("send").checked).toBe(false);
    const alert = container.querySelector('[role="alert"]');
    if (alert === null) throw new Error("capability error did not render");
    expect(alert.textContent).toBe("no such peer");
  });

  it("does not toggle capabilities on a daemon peer", async () => {
    vi.mocked(devicesList).mockResolvedValue(
      replyWith({ peers: [{ ...CLIENT_PEER, role: "daemon" }] }),
    );
    await renderPanel();

    expect(container.querySelectorAll('input[type="checkbox"]')).toHaveLength(0);
    expect(container.textContent).toContain("scoped by the daemon");
  });

  it("asks for a second click before revoking", async () => {
    vi.mocked(devicesList).mockResolvedValue(replyWith({ peers: [CLIENT_PEER] }));
    await renderPanel();

    await act(async () => {
      buttonByText("Revoke").click();
      await Promise.resolve();
    });
    expect(peerRevoke).not.toHaveBeenCalled();
    expect(container.textContent).toContain(
      "Revoking stops this device reaching this one. It can come back only with a new pairing code.",
    );

    await act(async () => {
      buttonByText("Revoke now").click();
      await Promise.resolve();
    });
    expect(peerRevoke).toHaveBeenCalledWith(CLIENT_PEER.deviceId);
  });

  it("uses the stronger copy for a lost or stolen device", async () => {
    vi.mocked(devicesList).mockResolvedValue(replyWith({ peers: [CLIENT_PEER] }));
    await renderPanel();

    await act(async () => {
      buttonByText("Lost or stolen device").click();
      await Promise.resolve();
    });

    expect(container.textContent).toContain(
      "Revokes this device now, closes its connections, and records it in the audit log.",
    );
    await act(async () => {
      buttonByText("Revoke now").click();
      await Promise.resolve();
    });
    expect(peerRevoke).toHaveBeenCalledWith(CLIENT_PEER.deviceId);
  });

  it("keeps the last good reply on screen when a poll fails", async () => {
    vi.mocked(devicesList)
      .mockResolvedValueOnce(replyWith({ peers: [CLIENT_PEER] }))
      .mockRejectedValue({ code: "io", message: "daemon connection was lost" });
    vi.useFakeTimers();
    await renderPanel();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_000);
    });

    expect(container.textContent).toContain("daemon connection was lost");
    expect(container.textContent).toContain("Xiaomi 14");
  });

  it("shows a failure before the first reply instead of an empty device list", async () => {
    vi.mocked(devicesList).mockRejectedValue({
      code: "io",
      message: "daemon connection was lost",
    });
    await renderPanel();

    const alert = container.querySelector('[role="alert"]');
    if (alert === null) throw new Error("load failure did not render");
    expect(alert.textContent).toContain("daemon connection was lost");
    expect(buttonByText("Retry")).toBeTruthy();
  });
});

describe("device helpers", () => {
  it("groups fingerprints and codes in fours", () => {
    expect(groupFingerprint(FINGERPRINT)).toBe("0a1b 2c3d 4e5f 6071 8293 a4b5 c6d7 e8f9");
    expect(groupFingerprint("ABCD2345")).toBe("ABCD 2345");
    // A short tail is still shown rather than dropped.
    expect(groupFingerprint("abcde")).toBe("abcd e");
    expect(groupFingerprint("")).toBe("");
  });

  it("keeps only the unambiguous characters, uppercase, at most eight", () => {
    expect(sanitizePairingCode("ab01io")).toBe("ab".toUpperCase());
    expect(sanitizePairingCode("ABCD2345XY")).toBe("ABCD2345");
    expect(sanitizePairingCode("a b-c_d")).toBe("ABCD");
  });

  it("formats a countdown as m:ss and never goes negative", () => {
    expect(formatDuration(300_000)).toBe("5:00");
    expect(formatDuration(59_400)).toBe("1:00");
    expect(formatDuration(1)).toBe("0:01");
    expect(formatDuration(-5_000)).toBe("0:00");
  });

  it("describes how long ago something happened", () => {
    expect(relativeTime(NOW - 1_000, NOW)).toBe("just now");
    expect(relativeTime(NOW - 300_000, NOW)).toBe("5 min ago");
    expect(relativeTime(NOW - 7_200_000, NOW)).toBe("2 h ago");
    expect(relativeTime(NOW - 172_800_000, NOW)).toBe("2 d ago");
  });

  it("keeps the daemon's own reason text for a disabled remote", () => {
    expect(remoteLabel({ state: "enabled", reason: null })).toBe("Reachable on the tailnet");
    expect(remoteLabel({ state: "disabled", reason: null })).toBe("Remote off");
    expect(remoteLabel({ state: "disabled", reason: "Tailscale is not running" })).toBe(
      "Remote off · Tailscale is not running",
    );
    expect(remoteLabel({ state: "key_missing", reason: "gone" })).toBe(
      "Key missing · re-pair required",
    );
  });
});

describe("mock inventory", () => {
  function sourceFiles(directory: string): string[] {
    return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) return sourceFiles(path);
      // Tests are excluded: this guard file names the symbol on purpose, and
      // a test import of a deleted module fails the typecheck by itself.
      if (path.includes(".test.") || path.includes(".spec.")) return [];
      return entry.isFile() && (path.endsWith(".ts") || path.endsWith(".tsx")) ? [path] : [];
    });
  }

  it("has no MOCK_DEVICES symbol left anywhere in src", () => {
    // The mock was deleted, not hidden behind a re-export: the panel is wired
    // to the daemon's reply, and this is the check that says so.
    const offenders = sourceFiles("src").filter((path) =>
      readFileSync(path, "utf8").includes("MOCK_DEVICES"),
    );
    expect(offenders).toEqual([]);
  });
});
