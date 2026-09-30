import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { vi } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { TurnRail } from "./TurnRail";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

interface RailHarness {
  conversation: HTMLDivElement;
  content: HTMLDivElement;
  anchors: HTMLElement[];
  rerender: (items: readonly AgentChatItem[]) => void;
  dots: () => HTMLButtonElement[];
  unmount: () => void;
}

/** Two items per turn: a user bubble and the agent's answer after it. */
export function transcript(turnCount: number): AgentChatItem[] {
  const items: AgentChatItem[] = [];
  for (let index = 1; index <= turnCount; index += 1) {
    items.push({
      id: `u-${index}`,
      role: "user",
      text: `Question ${index}\nanother line`,
      messageId: null,
    });
    items.push({
      id: `a-${index}`,
      role: "assistant",
      text: `Answer ${index}`,
      messageId: null,
    });
  }
  return items;
}

export function mountRail(items: readonly AgentChatItem[]): RailHarness {
  const conversation = document.createElement("div");
  conversation.className = "workspace-conversation workspace-scroll";
  const content = document.createElement("div");
  content.className = "workspace-conversation-content";
  conversation.appendChild(content);
  document.body.appendChild(conversation);

  // The bubbles are the surface's to render; the rail only reads the
  // anchors they carry, so the harness mirrors that seam and nothing more.
  const anchors: HTMLElement[] = [];
  const anchoredIds = new Set<string>();
  const addAnchor = (id: string): void => {
    const anchor = document.createElement("div");
    anchor.className = "workspace-chat-user";
    anchor.setAttribute("data-turn-anchor", id);
    const bubble = document.createElement("div");
    bubble.className = "workspace-chat-bubble";
    anchor.appendChild(bubble);
    content.appendChild(anchor);
    anchors.push(anchor);
    anchoredIds.add(id);
  };
  for (const item of items) {
    if (item.role === "user") addAnchor(item.id);
  }
  const mountPoint = document.createElement("div");
  content.appendChild(mountPoint);

  const scrollRef: { current: HTMLDivElement | null } = { current: conversation };
  const contentRef: { current: HTMLDivElement | null } = { current: content };
  const root: Root = createRoot(mountPoint);
  const render = (next: readonly AgentChatItem[]): void => {
    // The surface mounts a row per user item; a turn list that grows
    // finds its anchor only if the harness grows one too. The set keeps
    // this query-free — the cost tests count every query.
    for (const item of next) {
      if (item.role === "user" && !anchoredIds.has(item.id)) addAnchor(item.id);
    }
    act(() => root.render(<TurnRail scrollRef={scrollRef} contentRef={contentRef} items={next} />));
  };
  render(items);

  return {
    conversation,
    content,
    anchors,
    rerender: render,
    dots: () => [...content.querySelectorAll<HTMLButtonElement>("button.turn-rail-dot")],
    unmount: () => {
      act(() => root.unmount());
      conversation.remove();
    },
  };
}

export function stubOverflow(
  conversation: HTMLDivElement,
  scrollHeight: number,
  clientHeight: number,
): void {
  Object.defineProperty(conversation, "scrollHeight", { value: scrollHeight, configurable: true });
  Object.defineProperty(conversation, "clientHeight", { value: clientHeight, configurable: true });
}

/** The scrollport's box in the viewport: `bottom` is the viewport's bottom
 * edge for the rule that reads the end of the scroll range. */
export function stubViewport(conversation: HTMLDivElement, bottom: number): void {
  conversation.getBoundingClientRect = () =>
    ({
      top: 0,
      bottom,
      left: 0,
      right: 0,
      width: 0,
      height: bottom,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    }) as DOMRect;
}

/** A bubble's geometry: its box in the transcript, its position in the
 * viewport, and — `rewrapPx` — how far opening the gutter pushes it down. */
export function stubAnchor(
  anchor: HTMLElement,
  conversation: HTMLDivElement,
  offsetTop: number,
  height: number,
  rewrapPx = 0,
): void {
  Object.defineProperty(anchor, "offsetTop", { get: () => offsetTop, configurable: true });
  Object.defineProperty(anchor, "offsetHeight", { value: height, configurable: true });
  anchor.getBoundingClientRect = () =>
    ({
      top:
        offsetTop -
        conversation.scrollTop +
        (conversation.classList.contains("has-turn-rail") ? rewrapPx : 0),
      left: 0,
      right: 0,
      bottom: 0,
      width: 0,
      height: 0,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    }) as DOMRect;
}

/** The bubble inside a user row: the width that decides whether the
 * preview card has canvas to open into. */
export function stubBubble(anchor: HTMLElement, width: number): void {
  const bubble = anchor.querySelector<HTMLElement>(".workspace-chat-bubble");
  if (bubble === null) throw new Error("the turn anchor holds no bubble");
  bubble.getBoundingClientRect = () =>
    ({
      top: 0,
      bottom: 0,
      left: 0,
      right: width,
      width,
      height: 0,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    }) as DOMRect;
}

/** The column a content box shows: `clientWidth` includes the rail's
 * gutter (TurnRail.css: `has-turn-rail … content { padding-left: 32px }`),
 * so tests think in column pixels. */
export function stubColumnWidth(content: HTMLElement, columnWidth: number): void {
  Object.defineProperty(content, "clientWidth", { value: columnWidth + 32, configurable: true });
}

class ResizeObserverStub {
  static instances: ResizeObserverStub[] = [];
  readonly observed: Element[] = [];
  private readonly callback: ResizeObserverCallback;
  constructor(callback: ResizeObserverCallback) {
    this.callback = callback;
    ResizeObserverStub.instances.push(this);
  }
  observe(target: Element): void {
    this.observed.push(target);
  }
  unobserve(): void {}
  disconnect(): void {
    this.observed.length = 0;
  }
  fire(): void {
    if (this.observed.length === 0) return;
    this.callback([], this);
  }
}

export function installResizeObserver(): void {
  ResizeObserverStub.instances = [];
  vi.stubGlobal("ResizeObserver", ResizeObserverStub as unknown as typeof ResizeObserver);
}

/** Delivers one resize to every live observer, the way the browser would
 * after a bubble, the column, or the pane changed size. */
export function fireResize(): void {
  act(() => {
    for (const instance of ResizeObserverStub.instances) instance.fire();
  });
}

let frames: Map<number, FrameRequestCallback>;

/** Replaces the browser's rAF with a queue the test drains by hand. */
export function installFrameStub(): void {
  frames = new Map();
  let nextFrameId = 1;
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback): number => {
    const id = nextFrameId;
    nextFrameId += 1;
    frames.set(id, callback);
    return id;
  });
  vi.stubGlobal("cancelAnimationFrame", (id: number): void => {
    frames.delete(id);
  });
}

function flushFrames(): void {
  act(() => {
    const pending = [...frames.values()];
    frames.clear();
    for (const callback of pending) callback(performance.now());
  });
}

export function scrollTo(conversation: HTMLDivElement, scrollTop: number): void {
  conversation.scrollTop = scrollTop;
  conversation.dispatchEvent(new Event("scroll"));
  flushFrames();
}
