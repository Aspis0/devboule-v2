// The recovered reopen bar: the workspace hands it down and the surface renders
// it above the transcript, in the extras wrapper that holds no row of its own
// when there is nothing to show.
// @vitest-environment happy-dom
import { resetRegistry } from "../../lib/agentSessionRegistry";
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { channelHarness } from "./sessionChannelHarness";

vi.mock("../../lib/tauri", async () => (await import("./sessionChannelHarness")).tauriMock);

import { AgentChatSurface } from "./AgentChatSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: Root | null = null;

beforeEach(() => {
  host = document.createElement("div");
  document.body.appendChild(host);
  channelHarness.emit = null;
  channelHarness.active = null;
  channelHarness.activeSubscriptionId = null;
  channelHarness.nextSubscriptionId = 41;
  channelHarness.deferNextAttach = false;
  channelHarness.releaseNextAttach = null;
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  document.body.replaceChildren();
  vi.clearAllMocks();
});

async function renderSurface(headerTrailing?: ReactNode): Promise<void> {
  root = createRoot(host);
  await act(async () => {
    root!.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId="agent-trailing-id"
        observedState={{ type: "live", generation: 1 }}
        headerTrailing={headerTrailing}
      />,
    );
  });
}

it("renders the trailing slot above the transcript, with no pane header row", async () => {
  await renderSurface(<span data-testid="trailing-probe">probe</span>);

  const probe = host.querySelector('[data-testid="trailing-probe"]');
  if (probe === null) throw new Error("trailing slot did not render");
  const extras = probe.closest(".workspace-agent-extras");
  expect(extras).not.toBeNull();
  const conversation = host.querySelector(".workspace-conversation");
  expect(extras?.compareDocumentPosition(conversation as Node)).toBe(
    Node.DOCUMENT_POSITION_FOLLOWING,
  );
  expect(host.querySelector(".workspace-agent-toolbar")).toBeNull();
});

it("renders no trailing control when the workspace passes none", async () => {
  await renderSurface();

  expect(host.querySelector('[data-testid="trailing-probe"]')).toBeNull();
  expect(host.querySelector(".workspace-agent-toolbar")).toBeNull();
});

it("draws no title, state or kebab: the surface draws no header controls", async () => {
  await renderSurface();

  expect(host.querySelector(".workspace-agent-title, .workspace-agent-status")).toBeNull();
  expect(host.querySelector(".pane-header-kebab")).toBeNull();
});

afterEach(() => resetRegistry());
