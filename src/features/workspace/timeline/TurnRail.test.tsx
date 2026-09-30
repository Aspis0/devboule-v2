// @vitest-environment happy-dom

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import {
  fireResize,
  installFrameStub,
  installResizeObserver,
  mountRail,
  scrollTo,
  stubAnchor,
  stubOverflow,
  stubViewport,
  transcript,
} from "./turnRailHarness";

beforeEach(() => {
  document.body.innerHTML = "";
  installFrameStub();
  installResizeObserver();
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("TurnRail", () => {
  describe("when it shows", () => {
    it("renders nothing while the transcript fits", () => {
      const rail = mountRail(transcript(2));
      expect(rail.content.querySelector("nav.turn-rail")).toBeNull();
      expect(rail.conversation.classList.contains("has-turn-rail")).toBe(false);
      rail.unmount();
    });

    it("gives a gutter and a rail only while the transcript overflows", () => {
      const items = transcript(2);
      const rail = mountRail(items);

      stubOverflow(rail.conversation, 2000, 500);
      rail.rerender([...items]);
      fireResize();
      expect(rail.conversation.classList.contains("has-turn-rail")).toBe(true);
      const nav = rail.content.querySelector("nav.turn-rail");
      expect(nav).not.toBeNull();
      expect(nav!.getAttribute("aria-label")).toBe("Turns");
      // The rail lives inside the content box, so its frame and the
      // bubbles' measuring frame are the same origin.
      expect(rail.content.contains(nav!)).toBe(true);
      expect(rail.content.querySelector(".turn-rail-thread")).not.toBeNull();

      stubOverflow(rail.conversation, 500, 500);
      rail.rerender([...items]);
      fireResize();
      expect(rail.content.querySelector("nav.turn-rail")).toBeNull();
      expect(rail.conversation.classList.contains("has-turn-rail")).toBe(false);
      rail.unmount();
    });

    it("toggles the shell gutter with the conversation, for the composer", () => {
      const items = transcript(2);
      const rail = mountRail(items);

      stubOverflow(rail.conversation, 2000, 500);
      rail.rerender([...items]);
      fireResize();
      expect(rail.conversation.classList.contains("has-turn-rail")).toBe(true);
      expect(rail.shell.classList.contains("has-turn-rail")).toBe(true);

      stubOverflow(rail.conversation, 500, 500);
      rail.rerender([...items]);
      fireResize();
      expect(rail.shell.classList.contains("has-turn-rail")).toBe(false);
      rail.unmount();
    });

    it("removes the gutter from the conversation and the shell on unmount", () => {
      const items = transcript(2);
      const rail = mountRail(items);
      stubOverflow(rail.conversation, 2000, 500);
      rail.rerender([...items]);
      fireResize();
      expect(rail.conversation.classList.contains("has-turn-rail")).toBe(true);
      expect(rail.shell.classList.contains("has-turn-rail")).toBe(true);

      rail.unmount();
      expect(rail.conversation.classList.contains("has-turn-rail")).toBe(false);
      expect(rail.shell.classList.contains("has-turn-rail")).toBe(false);
    });

    it("stays absent on an overflowing transcript with no user turn", () => {
      const items: AgentChatItem[] = [
        { id: "a-1", role: "assistant", text: "no question yet", messageId: null },
      ];
      const rail = mountRail(items);
      stubOverflow(rail.conversation, 2000, 500);
      rail.rerender([...items]);
      fireResize();
      expect(rail.content.querySelector("nav.turn-rail")).toBeNull();
      expect(rail.conversation.classList.contains("has-turn-rail")).toBe(false);
      rail.unmount();
    });
  });

  describe("the gutter's scroll position", () => {
    it("holds the topmost visible bubble in place while the gutter opens", () => {
      const items = transcript(2);
      const rail = mountRail(items);
      stubOverflow(rail.conversation, 2000, 500);
      stubAnchor(rail.anchors[0], rail.conversation, 100, 40, 8);
      stubAnchor(rail.anchors[1], rail.conversation, 900, 40, 8);
      rail.conversation.scrollTop = 500;

      rail.rerender([...items]);
      fireResize();

      // The narrower column rewraps 8 px of content above the top; the
      // compensation scrolls by exactly that, so the view does not jump.
      expect(rail.conversation.classList.contains("has-turn-rail")).toBe(true);
      expect(rail.conversation.scrollTop).toBe(508);
      rail.unmount();
    });

    it("leaves scrollTop alone on a geometry tick with the gutter already open", () => {
      const items = transcript(2);
      const rail = mountRail(items);
      stubOverflow(rail.conversation, 2000, 500);
      stubAnchor(rail.anchors[0], rail.conversation, 100, 40, 8);
      stubAnchor(rail.anchors[1], rail.conversation, 900, 40, 8);
      rail.conversation.scrollTop = 500;

      rail.rerender([...items]);
      fireResize();
      expect(rail.conversation.classList.contains("has-turn-rail")).toBe(true);
      expect(rail.conversation.scrollTop).toBe(508);

      // Content below moved the last bubble, so the observer raises a tick
      // and the rail re-measures — but the gutter needs no transition and
      // the view must not jump a second time.
      rail.conversation.scrollTop = 500;
      stubAnchor(rail.anchors[1], rail.conversation, 920, 40, 8);
      fireResize();
      expect(rail.conversation.classList.contains("has-turn-rail")).toBe(true);
      expect(rail.conversation.scrollTop).toBe(500);
      rail.unmount();
    });

    it("leaves the view alone at the top of the transcript", () => {
      const items = transcript(2);
      const rail = mountRail(items);
      stubOverflow(rail.conversation, 2000, 500);
      stubAnchor(rail.anchors[0], rail.conversation, 100, 40, 8);
      stubAnchor(rail.anchors[1], rail.conversation, 900, 40, 8);
      rail.conversation.scrollTop = 0;

      rail.rerender([...items]);
      fireResize();
      expect(rail.conversation.scrollTop).toBe(0);

      stubOverflow(rail.conversation, 500, 500);
      rail.rerender([...items]);
      fireResize();
      expect(rail.conversation.scrollTop).toBe(0);
      rail.unmount();
    });
  });

  describe("the current turn", () => {
    it("marks the turn at the top of the viewport and follows the reader", () => {
      const items = transcript(3);
      const rail = mountRail(items);
      stubOverflow(rail.conversation, 3000, 500);
      stubAnchor(rail.anchors[0], rail.conversation, 100, 40);
      stubAnchor(rail.anchors[1], rail.conversation, 400, 40);
      stubAnchor(rail.anchors[2], rail.conversation, 700, 40);
      rail.rerender([...items]);
      fireResize();

      let dots = rail.dots();
      expect(dots).toHaveLength(3);
      // Above the first bubble nothing is at the top, so turn 1 is current.
      expect(dots[0].getAttribute("aria-current")).toBe("true");
      // The dot sits at its bubble's centre: 100 + 40 / 2.
      expect(dots[0].parentElement?.style.top).toBe("120px");

      scrollTo(rail.conversation, 450);
      dots = rail.dots();
      expect(dots.filter((dot) => dot.getAttribute("aria-current") === "true")).toHaveLength(1);
      expect(dots[1].getAttribute("aria-current")).toBe("true");

      scrollTo(rail.conversation, 900);
      expect(rail.dots()[2].getAttribute("aria-current")).toBe("true");
      rail.unmount();
    });

    it("labels every dot with its position and title", () => {
      const items = transcript(2);
      const rail = mountRail(items);
      stubOverflow(rail.conversation, 2000, 500);
      stubAnchor(rail.anchors[0], rail.conversation, 100, 40);
      stubAnchor(rail.anchors[1], rail.conversation, 900, 40);
      rail.rerender([...items]);
      fireResize();

      const dots = rail.dots();
      expect(dots.map((dot) => dot.getAttribute("aria-label"))).toEqual([
        "Turn 1 of 2: Question 1",
        "Turn 2 of 2: Question 2",
      ]);
      expect(dots[0].querySelector(".turn-rail-preview-title")?.textContent).toBe("Question 1");
      rail.unmount();
    });

    /** Twelve turns with the last viewport holding turns 10–12 (tops
     * −200/+50/+200 at max scroll): turn 11's bubble at 11 950 cannot
     * reach the top edge — max scrollTop is 11 900. */
    function readyClamped() {
      const items = transcript(12);
      const rail = mountRail(items);
      stubOverflow(rail.conversation, 12_400, 500);
      stubViewport(rail.conversation, 500);
      rail.anchors.forEach((anchor, index) => {
        const offsetTop = index < 9 ? 500 + index * 500 : 11_700 + (index - 9) * 250;
        stubAnchor(anchor, rail.conversation, offsetTop, 40);
      });
      rail.rerender([...items]);
      fireResize();
      scrollTo(rail.conversation, 11_900);
      return rail;
    }

    it("pins the clicked dot when the jump's landing clamps at the end", () => {
      const rail = readyClamped();

      // The clamped landing: a browser fires this scroll after the jump.
      act(() => rail.dots()[10].click());
      scrollTo(rail.conversation, 11_900);

      // Dot 11 is current — not dot 12, whose bubble is inside the view too.
      expect(rail.dots()[10].getAttribute("aria-current")).toBe("true");
      expect(rail.dots().filter((dot) => dot.getAttribute("aria-current") === "true")).toHaveLength(
        1,
      );
      rail.unmount();
    });

    it("hands the current turn back to the top-edge rule on a wheel", () => {
      const rail = readyClamped();

      act(() => rail.dots()[11].click());
      scrollTo(rail.conversation, 11_900);
      // The pin holds the last dot through the jump's own scroll events.
      expect(rail.dots()[11].getAttribute("aria-current")).toBe("true");

      // The reader's scroll intent releases it: the spec rule (the last
      // bubble at or above the top edge) says turn 10 from these tops
      // (−200/+50/+200) — the previous dot, not the one that was pinned.
      act(() => {
        rail.conversation.dispatchEvent(new Event("wheel", { bubbles: true }));
      });
      expect(rail.dots()[9].getAttribute("aria-current")).toBe("true");
      expect(rail.dots().filter((dot) => dot.getAttribute("aria-current") === "true")).toHaveLength(
        1,
      );
      rail.unmount();
    });

    it("keeps the pin through the press that starts a click", () => {
      const rail = readyClamped();
      act(() => rail.dots()[11].click());
      expect(rail.dots()[11].getAttribute("aria-current")).toBe("true");

      // The real sequence a browser sends for a click on another dot:
      // pointerdown first. The press comes from the rail's own surface —
      // it is the interaction that moves the pin, not scroll intent.
      act(() => {
        rail.dots()[8].dispatchEvent(new Event("pointerdown", { bubbles: true }));
      });
      expect(rail.dots()[11].getAttribute("aria-current")).toBe("true");
      expect(rail.dots().filter((dot) => dot.getAttribute("aria-current") === "true")).toHaveLength(
        1,
      );

      act(() => {
        rail.dots()[8].dispatchEvent(new Event("pointerup", { bubbles: true }));
      });
      expect(rail.dots()[11].getAttribute("aria-current")).toBe("true");

      // Only the click's jump moves the pin, to the dot that was clicked.
      act(() => rail.dots()[8].click());
      expect(rail.dots()[8].getAttribute("aria-current")).toBe("true");
      rail.unmount();
    });

    it("releases the pin when focus enters the transcript, not between dots", () => {
      const rail = readyClamped();
      act(() => rail.dots()[11].click());
      expect(rail.dots()[11].getAttribute("aria-current")).toBe("true");

      // A keyboard reader tabs into a bubble: focus (and the browser's
      // scroll-into-view) took the view somewhere the pin no longer names.
      act(() => {
        rail.anchors[2].dispatchEvent(new Event("focusin", { bubbles: true }));
      });
      expect(rail.dots()[9].getAttribute("aria-current")).toBe("true");
      expect(rail.dots().filter((dot) => dot.getAttribute("aria-current") === "true")).toHaveLength(
        1,
      );

      // Focus moving between the rail's own dots is not a release.
      act(() => rail.dots()[11].click());
      act(() => {
        rail.dots()[0].dispatchEvent(new Event("focusin", { bubbles: true }));
      });
      expect(rail.dots()[11].getAttribute("aria-current")).toBe("true");
      rail.unmount();
    });
  });

  describe("jumping", () => {
    function readyToJump(turnCount: number) {
      const items = transcript(turnCount);
      const rail = mountRail(items);
      stubOverflow(rail.conversation, 4000, 500);
      rail.anchors.forEach((anchor, index) =>
        stubAnchor(anchor, rail.conversation, 100 + index * 400, 40),
      );
      rail.rerender([...items]);
      fireResize();
      return rail;
    }

    it("clicking a dot scrolls that turn's bubble into view", () => {
      const rail = readyToJump(2);
      const firstJump = vi.spyOn(rail.anchors[0], "scrollIntoView");
      const secondJump = vi.spyOn(rail.anchors[1], "scrollIntoView");
      const dots = rail.dots();

      act(() => dots[1].click());

      expect(secondJump).toHaveBeenCalledTimes(1);
      expect(secondJump).toHaveBeenCalledWith({ behavior: "smooth", block: "start" });
      expect(firstJump).not.toHaveBeenCalled();
      rail.unmount();
    });

    it("jumps without the smooth scroll when reduced motion is preferred", () => {
      vi.spyOn(window, "matchMedia").mockReturnValue({
        matches: true,
      } as unknown as MediaQueryList);
      const rail = readyToJump(2);
      const jump = vi.spyOn(rail.anchors[0], "scrollIntoView");

      act(() => rail.dots()[0].click());

      expect(jump).toHaveBeenCalledWith({ behavior: "auto", block: "start" });
      rail.unmount();
    });

    it("jumps on Enter and Space, keeping focus on the dot", () => {
      const rail = readyToJump(2);
      const jump = vi.spyOn(rail.anchors[0], "scrollIntoView");
      const dot = rail.dots()[0];
      dot.focus();

      const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
      act(() => dot.dispatchEvent(enter));
      expect(jump).toHaveBeenCalledTimes(1);
      expect(enter.defaultPrevented).toBe(true);

      const space = new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true });
      act(() => dot.dispatchEvent(space));
      expect(jump).toHaveBeenCalledTimes(2);
      expect(jump).toHaveBeenLastCalledWith({ behavior: "smooth", block: "start" });
      // The rail is a list of stops: the reader keeps moving through it.
      expect(document.activeElement).toBe(dot);
      rail.unmount();
    });
  });

  describe("geometry the rail did not ask for", () => {
    it("re-measures every dot when a bubble resize moves the transcript", () => {
      const items = transcript(3);
      const rail = mountRail(items);
      stubOverflow(rail.conversation, 6000, 500);
      stubAnchor(rail.anchors[0], rail.conversation, 100, 40);
      stubAnchor(rail.anchors[1], rail.conversation, 900, 40);
      stubAnchor(rail.anchors[2], rail.conversation, 1700, 40);
      rail.rerender([...items]);
      fireResize();
      expect(rail.dots()[2].parentElement?.style.top).toBe("1720px");

      // Content above the bubbles grew (an image decoded, the pane
      // rewrapped): every bubble moved down, so every dot must follow.
      stubAnchor(rail.anchors[0], rail.conversation, 300, 40);
      stubAnchor(rail.anchors[1], rail.conversation, 1100, 40);
      stubAnchor(rail.anchors[2], rail.conversation, 1900, 40);
      fireResize();
      expect(rail.dots()[2].parentElement?.style.top).toBe("1920px");
      expect(rail.dots()[0].parentElement?.style.top).toBe("320px");
      rail.unmount();
    });
  });

  describe("the cached turn elements", () => {
    it("skips the turn whose bubble is gone instead of misplacing its dot", () => {
      const items = transcript(2);
      const rail = mountRail(items);
      stubOverflow(rail.conversation, 2000, 500);
      stubAnchor(rail.anchors[0], rail.conversation, 100, 40);
      stubAnchor(rail.anchors[1], rail.conversation, 900, 40);
      rail.rerender([...items]);
      fireResize();
      expect(rail.dots()).toHaveLength(2);

      rail.anchors[1].remove();
      rail.rerender([...items]);
      fireResize();

      const dots = rail.dots();
      expect(dots).toHaveLength(1);
      expect(dots[0].getAttribute("aria-label")).toContain("Turn 1 of 2");
      rail.unmount();
    });

    it("rebuilds the cache when a middle anchor's node is replaced", () => {
      const items = transcript(4);
      const rail = mountRail(items);
      stubOverflow(rail.conversation, 6_000, 500);
      rail.anchors.forEach((anchor, index) =>
        stubAnchor(anchor, rail.conversation, 100 + index * 600, 40),
      );
      rail.rerender([...items]);
      fireResize();
      expect(rail.dots()[1].parentElement?.style.top).toBe("720px");

      // A replaced middle row: the new node measures 1 200. In a browser
      // the detached old node reads 0, and the cache would park the dot
      // there — on zeros that never change, so no re-measure would follow.
      const replacement = document.createElement("div");
      replacement.className = "workspace-chat-user";
      replacement.setAttribute("data-turn-anchor", "u-2");
      const bubble = document.createElement("div");
      bubble.className = "workspace-chat-bubble";
      replacement.appendChild(bubble);
      rail.anchors[1].replaceWith(replacement);
      stubAnchor(replacement, rail.conversation, 1_200, 40);

      // Move the last bubble so the observer's probe ticks and a pass runs.
      stubAnchor(rail.anchors[3], rail.conversation, 900, 40);
      fireResize();

      expect(rail.dots()[1].parentElement?.style.top).toBe("1220px");
      rail.unmount();
    });
  });
});
