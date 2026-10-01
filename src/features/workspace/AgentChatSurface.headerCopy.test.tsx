// @vitest-environment happy-dom

import { act } from "react";
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

it("renders the agent surface's session ID copy row without cwd or close actions", async () => {
  root = createRoot(host);
  await act(async () => {
    root!.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId="agent-copy-id"
        title="Human title"
        observedState={{ type: "live", generation: 1 }}
      />,
    );
  });
  const kebab = host.querySelector<HTMLButtonElement>(".pane-header-kebab");
  expect(kebab).not.toBeNull();
  await act(async () => kebab!.click());
  expect(
    [...document.querySelectorAll('.pane-header-menu [role="menuitem"]')].map(
      (row) => row.textContent,
    ),
  ).toEqual(["Copy session ID"]);
});
