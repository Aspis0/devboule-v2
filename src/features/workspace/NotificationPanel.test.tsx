// The Notifications tab through its own store: the empty state it starts each
// launch with, the rows a raise lists, the read-on-open that clears the badge,
// and the row click that opens the session through the activation export.
// @vitest-environment happy-dom
import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Attention, AttentionReason, Session } from "../../types/ipc";
import { activateAttentionTarget } from "./attentionActivation";
import { forgetAttentionFor } from "./attentionNotice";
import {
  notificationRows,
  recordAttentionRaise,
  resetNotificationsForTests,
  useNotificationFeed,
} from "./notificationCenter";
import { NotificationPanel } from "./NotificationPanel";

vi.mock("./attentionActivation", () => ({
  activateAttentionTarget: vi.fn(),
}));

function raise(reason: AttentionReason, atMs: number): Attention {
  return { reason, atMs };
}

function session(id: string, workspaceId: string | null): Session {
  return {
    id,
    workspaceId,
    title: `${id} title`,
    state: { type: "idle", generation: 1 },
  } as unknown as Session;
}

let container: HTMLElement;
let root: Root;

function render(element: React.ReactElement): void {
  act(() => {
    root.render(<StrictMode>{element}</StrictMode>);
  });
}

beforeEach(() => {
  resetNotificationsForTests();
  forgetAttentionFor(new Set());
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.clearAllMocks();
});

function rows(): HTMLElement[] {
  return [...container.querySelectorAll<HTMLElement>(".notifications-row")];
}

describe("the notifications panel", () => {
  it("declares its empty start while the feed holds nothing", () => {
    render(<NotificationPanel />);

    const empty = container.querySelector(".notifications-empty")?.textContent;
    expect(empty).toContain("starts empty each launch");
    expect(empty).toContain("keeps the last 50");
    expect(rows()).toHaveLength(0);
  });

  it("lists raises newest first, with their word and session name", () => {
    act(() => {
      recordAttentionRaise(session("s1", null), raise("finished", 1_000), "s1 title");
      recordAttentionRaise(session("s2", "w2"), raise("permission", 2_000), "s2 title");
    });
    render(<NotificationPanel />);

    expect(rows()).toHaveLength(2);
    expect(rows()[0].textContent).toContain("s2 title");
    expect(rows()[0].textContent).toContain("Needs your approval");
    expect(rows()[1].textContent).toContain("s1 title");
    expect(rows()[1].textContent).toContain("Done");
  });

  it("opens the session a row names through the activation export", () => {
    act(() => {
      recordAttentionRaise(session("s1", "w2"), raise("error", 1_000), "s1 title");
    });
    render(<NotificationPanel />);

    act(() => {
      rows()[0].click();
    });
    expect(activateAttentionTarget).toHaveBeenCalledWith({ sessionId: "s1", workspaceId: "w2" });
  });

  it("grows the badge's unread count on a raise", () => {
    const reads: number[] = [];
    function Badge(): React.ReactNode {
      const { unreadCount } = useNotificationFeed();
      reads.push(unreadCount);
      return null;
    }
    render(<Badge />);
    expect(reads[reads.length - 1]).toBe(0);

    act(() => {
      recordAttentionRaise(session("s1", null), raise("finished", 1_000), "s1 title");
    });
    expect(reads[reads.length - 1]).toBe(1);
  });

  it("marks the feed read by being on screen, as the badge reads it", () => {
    const reads: number[] = [];
    function Badge(): React.ReactNode {
      const { unreadCount } = useNotificationFeed();
      reads.push(unreadCount);
      return null;
    }
    act(() => {
      recordAttentionRaise(session("s1", null), raise("finished", 1_000), "s1 title");
    });
    // The badge alone still sees the raise unread; the panel beside it reads it.
    render(
      <>
        <Badge />
        <NotificationPanel />
      </>,
    );

    expect(notificationRows().every((row) => row.read)).toBe(true);
    expect(reads[reads.length - 1]).toBe(0);
  });

  it("reads a raise that lands while the panel is on screen, so no dot returns", () => {
    act(() => {
      recordAttentionRaise(session("s1", null), raise("finished", 1_000), "s1 title");
    });
    render(<NotificationPanel />);

    act(() => {
      recordAttentionRaise(session("s2", null), raise("error", 2_000), "s2 title");
    });

    // The user watched the row land in the open tab: every row is read, and
    // unreadCount — what the bell's badge reads — stays zero after leaving.
    expect(notificationRows().map((row) => row.sessionId)).toEqual(["s1", "s2"]);
    expect(notificationRows().every((row) => row.read)).toBe(true);
  });

  it("says one polite word for a raise that lands while the panel is open", () => {
    render(<NotificationPanel />);

    act(() => {
      recordAttentionRaise(session("s2", null), raise("error", 2_000), "s2 title");
    });

    const live = container.querySelector('[aria-live="polite"]');
    expect(live?.textContent).toBe("New activity: s2 title, Failed");
  });
});
