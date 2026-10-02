// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daemonStatus, devicesList } from "../../../lib/tauri";
import type { DaemonStatus, DevicesReply, PeerRow } from "../../../types/ipc";
import { Sidebar, type SidebarProps } from "./Sidebar";

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
    deviceId: "peer-1",
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

function sidebarProps(overrides: Partial<SidebarProps> = {}): SidebarProps {
  return {
    width: 280,
    collapsed: false,
    onCollapsedChange: vi.fn(),
    onResizeStart: vi.fn(),
    onResizeKeyDown: vi.fn(),
    resizeMin: 200,
    resizeMax: 480,
    historyOpen: false,
    onToggleHistory: vi.fn(),
    history: {
      searchValue: "",
      projects: [],
      branches: new Map(),
      onWorkspaceIdsChange: vi.fn(),
      selectedSessionId: null,
      onSearchChange: vi.fn(),
      onReopen: vi.fn(),
      onReopenAgent: vi.fn(),
    },
    searchValue: "",
    onSearchChange: vi.fn(),
    onAddProject: vi.fn(),
    addProjectRef: { current: null },
    tree: {
      projects: [],
      loading: true,
      error: null,
      providerError: null,
      selectedWorkspace: null,
      onRetryProjects: vi.fn(),
      onSelectWorkspace: vi.fn(),
      onNewWorkspace: vi.fn(),
      onRenameWorkspace: vi.fn(async () => null),
      onDeleteWorkspace: vi.fn(),
      providerMenuAnchorProjectId: null,
      providerMenu: null,
      stats: new Map(),
    },
    daemon: CONNECTED,
    daemonNote: null,
    ...overrides,
  };
}

/** What a node is, by name and class, so a header's exact shape can be pinned. */
function shape(node: ChildNode): string {
  if (node.nodeType === Node.TEXT_NODE) return `text:${node.textContent}`;
  if (node.nodeType !== Node.ELEMENT_NODE) return `node:${node.nodeType}`;
  const element = node as Element;
  return `${element.tagName.toLowerCase()}.${element.className}`;
}

describe("the sidebar's host sections", () => {
  let container: HTMLDivElement;
  let root: Root;

  /**
   * The poll answers in microtasks, and its interval only fires when a test
   * asks for it, so let those microtasks run before reading the DOM.
   */
  async function settle(): Promise<void> {
    await act(async () => {
      for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();
    });
  }

  async function render(overrides: Partial<SidebarProps> = {}): Promise<void> {
    root = createRoot(container);
    await act(async () => {
      root.render(<Sidebar {...sidebarProps(overrides)} />);
    });
    await settle();
  }

  function sections(): HTMLElement[] {
    return [...container.querySelectorAll<HTMLElement>(".sidebar-host-section")];
  }

  function headTexts(): (string | null)[] {
    return sections().map(
      (section) => section.querySelector<HTMLElement>(".sidebar-host-head")?.textContent ?? null,
    );
  }

  function hostHead(): HTMLElement {
    const head = container.querySelector<HTMLElement>(".sidebar-host-head");
    if (head === null) throw new Error("the host header did not render");
    return head;
  }

  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(daemonStatus).mockResolvedValue(CONNECTED);
    vi.mocked(devicesList).mockResolvedValue(reply([]));
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.useRealTimers();
  });

  it("with this PC alone, draws no section and nothing that folds", async () => {
    await render();

    // The markup itself is pinned node for node in Sidebar.loneHost.test.tsx;
    // what is left here is the behaviour that a markup dump cannot carry.
    expect(sections()).toHaveLength(0);
    const head = hostHead();
    expect(head.tagName).toBe("DIV");
    expect(head.getAttribute("aria-expanded")).toBeNull();
    expect(head.textContent).toBe("This PC");
  });

  it("with this PC alone, an offline daemon still greys the dot and drops no word", async () => {
    // The local host's status is the `daemon` prop the workspace surface reads
    // from the same poll, not a second source.
    await render({ daemon: { ...CONNECTED, state: "disconnected" } });

    expect([...hostHead().childNodes].map(shape)).toEqual([
      "svg.sidebar-host-icon",
      "text:This PC",
      "span.sidebar-top-spacer",
      "span.workspace-status-dot workspace-dot-border",
    ]);
  });

  it("gives a daemon peer a section and a client peer nothing", async () => {
    vi.mocked(devicesList).mockResolvedValue(
      reply([
        peer({ deviceId: "peer-daemon", displayName: "Studio" }),
        peer({ deviceId: "peer-client", displayName: "Phone", role: "client" }),
      ]),
    );

    await render();

    expect(headTexts()).toEqual(["This PConline", "Studioonline"]);
    expect(container.textContent).not.toContain("Phone");
  });

  it("puts the local host first and the remote ones in name order", async () => {
    vi.mocked(devicesList).mockResolvedValue(
      reply([
        peer({ deviceId: "peer-z", displayName: "Zeta" }),
        peer({ deviceId: "peer-a", displayName: "alpha" }),
        peer({ deviceId: "peer-m", displayName: "Mika" }),
      ]),
    );

    await render();

    expect(headTexts()).toEqual(["This PConline", "alphaonline", "Mikaonline", "Zetaonline"]);
  });

  it("keeps the workspace tree in the local section and one honest line in a remote one", async () => {
    vi.mocked(devicesList).mockResolvedValue(reply([peer({ displayName: "Studio" })]));

    await render();

    expect(sections()[0]?.textContent).toContain("Loading projects…");
    expect(sections()[1]?.querySelector(".sidebar-host-note")?.textContent).toBe(
      "This host's workspaces are not available in this version.",
    );
  });

  it("shows each liveness as its own word and dot", async () => {
    vi.mocked(devicesList).mockResolvedValue(reply([peer({ displayName: "Studio" })]));

    await render();

    const online = sections()[1]?.querySelector<HTMLElement>(".sidebar-host-head");
    expect(online?.textContent).toContain("online");
    expect(online?.querySelector(".workspace-status-dot")?.className).toBe(
      "workspace-status-dot workspace-dot-green",
    );
  });

  it("shows an offline peer as offline", async () => {
    vi.mocked(devicesList).mockResolvedValue(
      reply([peer({ displayName: "Studio", online: false })]),
    );

    await render();

    const offline = sections()[1]?.querySelector<HTMLElement>(".sidebar-host-head");
    expect(offline?.textContent).toContain("offline");
    expect(offline?.querySelector(".workspace-status-dot")?.className).toBe(
      "workspace-status-dot workspace-dot-border",
    );
  });

  it("keeps the hosts and says it cannot tell after a failed poll", async () => {
    vi.mocked(devicesList).mockResolvedValue(reply([peer({ displayName: "Studio" })]));
    await render();

    vi.mocked(devicesList).mockRejectedValue(new Error("daemon unreachable"));
    await act(async () => {
      vi.advanceTimersByTime(2000);
    });
    await settle();

    expect(headTexts()).toEqual(["This PConline", "Studiounknown"]);
  });

  it("folds and unfolds a remote section from its own header", async () => {
    vi.mocked(devicesList).mockResolvedValue(reply([peer({ displayName: "Studio" })]));

    await render();

    const remoteHead = sections()[1]?.querySelector<HTMLButtonElement>(".sidebar-host-head");
    const localHead = sections()[0]?.querySelector<HTMLButtonElement>(".sidebar-host-head");
    if (remoteHead === null || localHead === null) throw new Error("a host header did not render");
    expect(remoteHead.tagName).toBe("BUTTON");
    expect(remoteHead.getAttribute("aria-expanded")).toBe("true");

    await act(async () => remoteHead.click());

    expect(remoteHead.getAttribute("aria-expanded")).toBe("false");
    expect(container.textContent).not.toContain("not available in this version");
    // The local section is its own state: folding a host folds that host.
    expect(sections()[0]?.textContent).toContain("Loading projects…");
    expect(localHead.getAttribute("aria-expanded")).toBe("true");

    await act(async () => remoteHead.click());

    expect(remoteHead.getAttribute("aria-expanded")).toBe("true");
    expect(container.textContent).toContain("not available in this version");
  });
});
