// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TurnStop } from "./TurnStop";
import type { UserTurn } from "./turnGrouping";

const turn: UserTurn = { id: "u-1", title: "Question 1" };

function box(top: number, height: number): DOMRect {
  return {
    top,
    height,
    bottom: top + height,
    left: 0,
    right: 0,
    width: 0,
    x: 0,
    y: top,
  } as DOMRect;
}

let root: Root | null = null;
let conversation: HTMLDivElement | null = null;

afterEach(() => {
  act(() => root?.unmount());
  conversation?.remove();
  root = null;
  conversation = null;
});

/** One dot in a transcript whose visible box runs from 0 to 100 px, with the
 * dot's box at `dotTop` and a 40 px card. `show` renders it open or closed. */
function renderStop(dotTop: number) {
  conversation = document.createElement("div");
  conversation.className = "workspace-conversation";
  document.body.appendChild(conversation);
  const host = document.createElement("div");
  conversation.appendChild(host);
  root = createRoot(host);
  const show = (isOpen: boolean) =>
    act(() => {
      root!.render(
        <TurnStop
          turn={turn}
          index={0}
          count={1}
          center={10}
          isCurrent={false}
          isOpen={isOpen}
          tabIndex={0}
          today="today"
          jumpTo={vi.fn()}
          openFromFocus={vi.fn()}
          pressStarted={vi.fn()}
          closePreview={vi.fn()}
        />,
      );
    });
  show(false);
  conversation.getBoundingClientRect = () => box(0, 100);
  const dot = host.querySelector<HTMLElement>(".turn-rail-dot")!;
  dot.getBoundingClientRect = () => box(dotTop, 24);
  const card = host.querySelector<HTMLElement>(".turn-rail-preview")!;
  card.getBoundingClientRect = () => box(0, 40);
  return { dot, card, show };
}

describe("the preview card's place in the transcript", () => {
  it("moves a card whose dot sits near the top edge down when the pointer comes to the dot", () => {
    const { dot, card } = renderStop(2);
    act(() => {
      dot.dispatchEvent(new Event("pointerover", { bubbles: true }));
    });
    expect(card.style.getPropertyValue("--card-shift")).toBe("14px");
  });

  it("moves a card whose dot sits near the bottom edge up when the card is opened", () => {
    const { card, show } = renderStop(95);
    show(true);
    expect(card.style.getPropertyValue("--card-shift")).toBe("-35px");
  });
});
