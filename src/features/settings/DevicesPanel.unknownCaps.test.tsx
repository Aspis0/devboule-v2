// @vitest-environment happy-dom
// Unknown capabilities survive an unrelated toggle, on and off.
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
import { DevicesPanel } from "./DevicesPanel";

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
  role: "client",
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
const UNKNOWN = "future_cap_not_in_table" as unknown as PeerRow["caps"][number];

function replyWith(peers: PeerRow[]): DevicesReply {
  return { selfInfo: SELF, peers, pending: [] };
}

describe("unknown capability round trip", () => {
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
    vi.mocked(peerSetCaps).mockResolvedValue({ ...PEER, caps: ["view", "send"] });
  });

  afterEach(async () => {
    if (root !== null) await act(async () => root?.unmount());
    root = null;
    container.remove();
    vi.clearAllMocks();
  });

  it("keeps an unknown cap when turning an unrelated switch on", async () => {
    vi.mocked(devicesList).mockResolvedValue(replyWith([{ ...PEER, caps: ["view", UNKNOWN] }]));
    await renderPanel();

    await act(async () => {
      checkboxByLabel("send").click();
      await Promise.resolve();
    });

    const sent = vi.mocked(peerSetCaps).mock.calls[0]?.[1] ?? [];
    expect(sent).toContain(UNKNOWN as string);
    expect(sent).toEqual(["view", "send", UNKNOWN]);
  });

  it("keeps an unknown cap when turning an unrelated switch off", async () => {
    vi.mocked(devicesList).mockResolvedValue(
      replyWith([{ ...PEER, caps: ["view", "send", UNKNOWN] }]),
    );
    await renderPanel();

    await act(async () => {
      checkboxByLabel("send").click();
      await Promise.resolve();
    });

    const sent = vi.mocked(peerSetCaps).mock.calls[0]?.[1] ?? [];
    expect(sent).toContain(UNKNOWN as string);
    expect(sent).toEqual(["view", UNKNOWN]);
  });
});
