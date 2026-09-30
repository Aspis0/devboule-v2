// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { channelHarness } from "../sessionChannelHarness";

vi.mock("../../../lib/tauri", async () => (await import("../sessionChannelHarness")).tauriMock);

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

import { AgentChatSurface } from "../AgentChatSurface";
import { fireResize, installResizeObserver } from "./turnRailHarness";

describe("the turn rail through the surface", () => {
  let container: HTMLDivElement;
  let root: Root | null;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    installResizeObserver();
    root = null;
    channelHarness.emit = null;
    channelHarness.active = null;
    channelHarness.activeSubscriptionId = null;
    channelHarness.nextSubscriptionId = 41;
    channelHarness.deferNextAttach = false;
  });

  afterEach(() => {
    const mounted = root;
    if (mounted !== null) act(() => mounted.unmount());
    container.remove();
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it("takes the surface's user bubbles as turns: anchor, gutter, dot", async () => {
    const surfaceRoot = createRoot(container);
    root = surfaceRoot;
    await act(async () => {
      surfaceRoot.render(
        <AgentChatSurface daemonState="connected" sessionId="rail-session" title="Agent" />,
      );
    });
    await act(async () => undefined);

    await act(async () => {
      channelHarness.active?.({
        type: "agent_user_message",
        author: "human",
        messageId: "user-1",
        text: "Line the transcript up\nand keep it there",
      });
      channelHarness.active?.({
        type: "agent_message",
        messageId: "answer-1",
        text: "Lined up.",
      });
    });

    const conversation = container.querySelector<HTMLElement>(".workspace-conversation");
    const content = container.querySelector<HTMLElement>(".workspace-conversation-content");
    if (conversation === null || content === null) throw new Error("transcript did not render");

    const anchor = content.querySelector<HTMLElement>("[data-turn-anchor]");
    if (anchor === null) throw new Error("the user bubble carries no turn anchor");
    expect(anchor.textContent).toContain("Line the transcript up");

    // The transcript fits: nothing rendered, no gutter.
    expect(content.querySelector("nav.turn-rail")).toBeNull();
    expect(conversation.classList.contains("has-turn-rail")).toBe(false);

    // The scrollport overflows; a resize delivery is where the rail reads it.
    Object.defineProperty(conversation, "scrollHeight", { value: 2000, configurable: true });
    Object.defineProperty(conversation, "clientHeight", { value: 500, configurable: true });
    await act(async () => {
      channelHarness.active?.({ type: "agent_message", messageId: "answer-2", text: "More." });
    });
    fireResize();

    const nav = content.querySelector<HTMLElement>("nav.turn-rail");
    expect(nav).not.toBeNull();
    expect(nav!.getAttribute("aria-label")).toBe("Turns");
    expect(conversation.classList.contains("has-turn-rail")).toBe(true);
    const dot = nav!.querySelector<HTMLButtonElement>("button.turn-rail-dot");
    if (dot === null) throw new Error("the rail rendered no dot");
    expect(dot.getAttribute("aria-label")).toBe("Turn 1 of 1: Line the transcript up");
    expect(dot.getAttribute("aria-current")).toBe("true");
  });

  it("carries the daemon's turn time onto the dot and the card", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 8, 30, 12, 0, 0));
    const surfaceRoot = createRoot(container);
    root = surfaceRoot;
    await act(async () => {
      surfaceRoot.render(
        <AgentChatSurface daemonState="connected" sessionId="time-session" title="Agent" />,
      );
    });
    await act(async () => undefined);

    const askedAt = new Date(2026, 8, 30, 9, 5);
    await act(async () => {
      channelHarness.active?.({
        type: "agent_user_message",
        author: "human",
        messageId: "user-time-1",
        messageKind: "composer",
        text: "Ask at nine",
        atMs: askedAt.getTime(),
      });
      channelHarness.active?.({
        type: "agent_message",
        messageId: "answer-time-1",
        text: "Answered.",
      });
    });

    const conversation = container.querySelector<HTMLElement>(".workspace-conversation");
    const content = container.querySelector<HTMLElement>(".workspace-conversation-content");
    if (conversation === null || content === null) throw new Error("transcript did not render");
    Object.defineProperty(conversation, "scrollHeight", { value: 2000, configurable: true });
    Object.defineProperty(conversation, "clientHeight", { value: 500, configurable: true });
    await act(async () => {
      channelHarness.active?.({ type: "agent_message", messageId: "answer-time-2", text: "More." });
    });
    fireResize();

    const dot = content.querySelector<HTMLButtonElement>("button.turn-rail-dot");
    if (dot === null) throw new Error("the rail rendered no dot");
    const expected = new Intl.DateTimeFormat(undefined, {
      hour: "2-digit",
      minute: "2-digit",
    }).format(askedAt);
    expect(dot.getAttribute("aria-label")).toBe(`Turn 1 of 1, ${expected}: Ask at nine`);
    expect(dot.querySelector(".turn-rail-preview-time")?.textContent).toBe(expected);
  });
});
