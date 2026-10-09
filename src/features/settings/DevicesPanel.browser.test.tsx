// @vitest-environment happy-dom
// The per-device `browser` switch: off until a person turns it on, and what
// it says it hands over.
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

import { devicesList, peerSetCaps } from "../../lib/tauri";
import type { DevicesReply, PeerRow, SelfInfo } from "../../types/ipc";
import { CAP_ORDER, DevicesPanel } from "./DevicesPanel";
import { resetPairingSession } from "./pairingSession";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const NOW = Date.now();
const SELF: SelfInfo = {
  deviceId: "self-1",
  displayName: "Self",
  publicKey: "cA==",
  keyFingerprint: "00".repeat(16),
  addresses: [],
  port: 47831,
  daemonVersion: "0.1.0",
  protocolVersion: 5,
  remote: { state: "enabled", reason: null },
};
const PEER: PeerRow = {
  deviceId: "peer-1",
  displayName: "Phone",
  publicKey: "cA==",
  keyFingerprint: "11".repeat(16),
  bindingKind: "tailnet",
  bindingNodeName: null,
  bindingLoginName: null,
  address: "100.64.0.1:47831",
  pairedAt: NOW - 1000,
  revokedAt: null,
  caps: ["view"],
  pairedByUser: "u",
  online: true,
};

/** The browser switch's label: the machine's pages it hands over, not its wire
 * name — and this machine's, not the paired device's. */
const BROWSER_LABEL = "drive this machine's browser tabs (read pages, click, type)";

function replyWith(peers: PeerRow[]): DevicesReply {
  return { selfInfo: SELF, peers, pending: [] };
}

describe("the browser capability switch", () => {
  let container: HTMLDivElement;
  let root: Root | null = null;

  function checkboxByLabel(label: string): HTMLInputElement {
    const found = Array.from(container.querySelectorAll<HTMLLabelElement>("label")).find((node) =>
      (node.textContent ?? "").startsWith(label),
    );
    if (found === undefined) throw new Error(`checkbox did not render: ${label}`);
    const field = found.querySelector<HTMLInputElement>('input[type="checkbox"]');
    if (field === null) throw new Error(`checkbox did not render: ${label}`);
    return field;
  }

  async function renderPanel(): Promise<void> {
    const mounted = createRoot(container);
    root = mounted;
    await act(async () => {
      mounted.render(<DevicesPanel />);
      await Promise.resolve();
    });
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(devicesList).mockResolvedValue(replyWith([]));
    vi.mocked(peerSetCaps).mockResolvedValue(PEER);
  });

  afterEach(async () => {
    if (root !== null) await act(async () => root?.unmount());
    resetPairingSession();
    root = null;
    container.remove();
    vi.clearAllMocks();
  });

  it("is one switch of the capability table, off on a pairing that never had it", async () => {
    vi.mocked(devicesList).mockResolvedValue(replyWith([PEER]));
    await renderPanel();

    expect(CAP_ORDER).toContain("browser");
    expect(checkboxByLabel(BROWSER_LABEL).checked).toBe(false);
  });

  it("turns it on for that device only, keeping the rest of its grants", async () => {
    vi.mocked(devicesList).mockResolvedValue(replyWith([{ ...PEER, caps: ["view", "search"] }]));
    await renderPanel();

    await act(async () => {
      checkboxByLabel(BROWSER_LABEL).click();
      await Promise.resolve();
    });

    expect(vi.mocked(peerSetCaps).mock.calls[0]?.[1]).toEqual(["view", "search", "browser"]);
  });

  it("renders the switch checked for a device already granted it", async () => {
    vi.mocked(devicesList).mockResolvedValue(replyWith([{ ...PEER, caps: ["view", "browser"] }]));
    await renderPanel();

    expect(checkboxByLabel(BROWSER_LABEL).checked).toBe(true);
  });

  // `search` has been in the default since 22 September 2026, so a device
  // paired then was born holding it: the copy may name the browser switch as
  // the exception and may not group search with it.
  it("says the browser switch is the one exception, without claiming search is off", async () => {
    vi.mocked(devicesList).mockResolvedValue(replyWith([PEER]));
    await renderPanel();

    const copy = Array.from(container.querySelectorAll("p.device-copy"))
      .map((node) => node.textContent ?? "")
      .find((text) => text.startsWith("A new pairing starts"));
    expect(copy).toContain("every switch on except browser");
    expect(copy).not.toMatch(/search and browser/);
  });
});
