// The agent header's trailing slot: the workspace hands the recovered reopen
// bar down and the header renders it on its own row, after the status word.
// @vitest-environment happy-dom
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
        title="Agent"
        observedState={{ type: "live", generation: 1 }}
        headerTrailing={headerTrailing}
      />,
    );
  });
}

it("renders the trailing slot inside the agent toolbar, before the kebab", async () => {
  await renderSurface(<span data-testid="trailing-probe">probe</span>);

  const toolbar = host.querySelector(".workspace-agent-toolbar");
  if (toolbar === null) throw new Error("agent toolbar did not render");
  const probe = toolbar.querySelector('[data-testid="trailing-probe"]');
  if (probe === null) throw new Error("trailing slot did not render in the toolbar");
  const children = [...toolbar.children];
  expect(children.at(-1)?.className).toContain("pane-header-kebab");
  expect(children.indexOf(probe)).toBeLessThan(children.length - 1);
});

it("renders no trailing control when the workspace passes none", async () => {
  await renderSurface();

  const toolbar = host.querySelector(".workspace-agent-toolbar");
  if (toolbar === null) throw new Error("agent toolbar did not render");
  expect(toolbar.querySelector('[data-testid="trailing-probe"]')).toBeNull();
  expect(toolbar.lastElementChild?.className).toContain("pane-header-kebab");
});
