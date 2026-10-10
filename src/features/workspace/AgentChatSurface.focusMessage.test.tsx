// @vitest-environment happy-dom
// A transcript search hit opens its session at the matching message: the row
// with that message id is scrolled into view, once the row has rendered.
import { resetRegistry } from "../../lib/agentSessionRegistry";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

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

describe("a search hit opens the transcript at its message", () => {
  let container: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;
  let scrolled: HTMLElement[];
  const realScroll = Element.prototype.scrollIntoView;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    channelHarness.active = null;
    scrolled = [];
    Element.prototype.scrollIntoView = function scrollIntoViewSpy(this: HTMLElement) {
      scrolled.push(this);
    };
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    Element.prototype.scrollIntoView = realScroll;
    vi.clearAllMocks();
  });

  function deliver(messageId: string, text: string): Promise<void> {
    return act(async () => {
      channelHarness.active?.({ type: "agent_message", messageId, text });
    });
  }

  it("scrolls to the row of the matching message once it renders", async () => {
    await act(async () => {
      root.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId="codex-agent"
          focusMessage={{ messageId: "m2", nonce: 1 }}
        />,
      );
    });
    await deliver("m1", "The first answer");
    expect(scrolled).toEqual([]);
    await deliver("m2", "The Kalsa build passed");
    expect(scrolled).toHaveLength(1);
    expect(scrolled[0].dataset.messageId).toBe("m2");
    expect(scrolled[0].textContent).toContain("The Kalsa build passed");
  });

  it("scrolls once per focus, and never to a message the session does not hold", async () => {
    await act(async () => {
      root.render(
        <AgentChatSurface
          daemonState="connected"
          sessionId="codex-agent"
          focusMessage={{ messageId: "gone", nonce: 1 }}
        />,
      );
    });
    await deliver("m1", "The first answer");
    await deliver("m2", "The second answer");
    expect(scrolled).toEqual([]);
  });
});

afterEach(() => resetRegistry());
