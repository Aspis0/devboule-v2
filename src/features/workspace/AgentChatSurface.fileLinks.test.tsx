// @vitest-environment happy-dom
// An agent message naming a workspace path renders it as a file link, and
// clicking the link hands the relative path to the surface's callback.
import { resetRegistry } from "../../lib/agentSessionRegistry";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ChatFileLinks } from "../../lib/chatFilePaths";

const channelHarness = vi.hoisted(() => ({
  active: null as ((event: unknown) => void) | null,
}));

vi.mock("../../lib/tauri", () => ({
  sessionsList: vi.fn(async () => []),
  createSessionChannel: vi.fn((onEvent: (event: unknown) => void) => {
    channelHarness.active = onEvent;
    return {};
  }),
  sessionAttach: vi.fn(async () => {
    await Promise.resolve();
    return 41;
  }),
  sessionDetach: vi.fn(async () => undefined),
  sessionSend: vi.fn(async () => undefined),
  sessionInterrupt: vi.fn(async () => undefined),
  sessionSetModel: vi.fn(async () => undefined),
  sessionSetMode: vi.fn(async () => undefined),
  isCommandError: () => false,
}));

import { AgentChatSurface } from "./AgentChatSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("file links in the agent transcript", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;
  const open = vi.fn();
  const fileLinks: ChatFileLinks = { root: "/home/u/repo", open };

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    channelHarness.active = null;
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  it("opens a message's path through the prop callback", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <AgentChatSurface daemonState="connected" sessionId="codex-agent" fileLinks={fileLinks} />,
      );
    });
    await act(async () => {
      channelHarness.active?.({
        type: "agent_message",
        messageId: "m1",
        text: "I edited src/app/main.tsx, see `src/lib/util.ts` and `src/lib/util.ts:12` too.",
      });
    });
    const buttons = container.querySelectorAll<HTMLButtonElement>("button.plan-markdown-file-link");
    expect(buttons.length).toBe(3);
    await act(async () => {
      buttons[0].click();
    });
    expect(open).toHaveBeenCalledTimes(1);
    expect(open).toHaveBeenCalledWith("src/app/main.tsx");
    expect(buttons[1].getAttribute("title")).toBe("src/lib/util.ts");
    expect(buttons[2].textContent).toBe("src/lib/util.ts:12");
    expect(buttons[2].title).toBe("src/lib/util.ts");
    await act(async () => buttons[2].click());
    expect(open).toHaveBeenCalledTimes(2);
    expect(open).toHaveBeenLastCalledWith("src/lib/util.ts");
  });
});

afterEach(() => resetRegistry());
