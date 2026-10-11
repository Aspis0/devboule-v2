// The card's sender label is the boundary between authenticated provenance
// and the local roster: far labels stay raw, while local ids may resolve.
// A sender the daemon verified reads as a plain line; only an unverified one is
// marked as such, and its words wait behind the line's click.
// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../lib/agentSession";
import { A2aMessageCard, type A2aNameSource } from "./A2aMessageCard";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

type A2aMessageItem = Extract<AgentChatItem, { role: "a2a_message" }>;

const DEVICE_ID = "7c9e6679-7425-40de-944b-e07fc1f90ae7";
const OTHER_DEVICE_ID = "1f0e6dad-f9ce-11ec-9d64-0242ac120002";

let container: HTMLDivElement;
let root: Root;

function message(fromAgent: string, origin: A2aMessageItem["origin"]): A2aMessageItem {
  return {
    id: "a2a-1",
    role: "a2a_message",
    fromAgent,
    origin,
    body: "hello from the other device",
  };
}

function names(sessionById: A2aNameSource["sessionById"]): A2aNameSource {
  return {
    sessionById,
    deviceNames: new Map([[DEVICE_ID, "Marco's phone"]]),
  };
}

async function renderCard(item: A2aMessageItem, nameSource: A2aNameSource) {
  root = createRoot(container);
  await act(async () => {
    root.render(<A2aMessageCard item={item} names={nameSource} />);
  });
}

function summaryOf(): HTMLElement {
  const summary = container.querySelector<HTMLElement>(".workspace-chat-activity-line");
  if (summary === null) throw new Error("the card drew no line");
  return summary;
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
});

describe("A2aMessageCard sender labels", () => {
  it("renders a matching far sender without looking its remainder up", async () => {
    const fromAgent = `peer:${DEVICE_ID}/s.msg.source`;
    await renderCard(
      message(fromAgent, { kind: "peer", device: DEVICE_ID }),
      names(
        new Map([
          ["s.msg.source", { id: "s.msg.source", title: "Local look-alike", kind: "claude" }],
        ]),
      ),
    );

    // A far sender is never looked up on this machine, so it has no title here.
    expect(summaryOf().textContent).toBe("Message from another agent");
    expect(summaryOf().textContent).not.toContain("Local look-alike");
    expect(summaryOf().textContent).not.toContain("s.msg.source");
    expect(summaryOf().getAttribute("title")).toBe(`${fromAgent} · ${DEVICE_ID}`);
  });

  it("shows a peer label whole when its device disagrees with the origin", async () => {
    const fromAgent = `peer:${OTHER_DEVICE_ID}/s.msg.source`;
    await renderCard(
      message(fromAgent, { kind: "peer", device: DEVICE_ID }),
      names(new Map([[fromAgent, { id: fromAgent, title: "Forged local name", kind: "claude" }]])),
    );

    expect(summaryOf().textContent).toBe("Message from another agent");
    expect(summaryOf().textContent).not.toContain(fromAgent);
    expect(summaryOf().textContent).not.toContain("Forged local name");
  });

  it("still resolves a local sender against the roster, and says nothing about its origin", async () => {
    await renderCard(
      message("s.local.1", { kind: "local" }),
      names(
        new Map([
          [
            "s.local.1",
            { id: "s.local.1", title: "worker", kind: "claude", displayName: "Local worker" },
          ],
        ]),
      ),
    );

    expect(summaryOf().textContent).toBe("Message from Local worker");
    expect(container.textContent).not.toContain("unverified");
    expect(container.textContent).not.toContain("this machine");
  });

  it("names a sender by the title the daemon gave on the event, never by its id", async () => {
    await renderCard(
      { ...message("s.kalsa.finish", { kind: "local" }), peerTitle: "Finish Kalsa website" },
      names(new Map()),
    );
    expect(summaryOf().textContent).toBe("Message from Finish Kalsa website");
    expect(summaryOf().textContent).not.toContain("s.kalsa.finish");
  });

  it("prefers the daemon's title to this machine's roster", async () => {
    await renderCard(
      { ...message("s.kalsa.finish", { kind: "local" }), peerTitle: "From the wire" },
      names(
        new Map([
          ["s.kalsa.finish", { id: "s.kalsa.finish", title: "Roster name", kind: "claude" }],
        ]),
      ),
    );
    expect(summaryOf().textContent).toBe("Message from From the wire");
  });

  it("never calls a verified peer's message unverified", async () => {
    await renderCard(
      message(`peer:${DEVICE_ID}/s.msg.source`, { kind: "peer", device: DEVICE_ID }),
      names(new Map()),
    );
    expect(container.textContent).not.toContain("unverified");
  });

  it("marks a sender the daemon could not verify, in the line itself", async () => {
    await renderCard(message("s.unknown.1", { kind: "unknown" }), names(new Map()));
    expect(summaryOf().textContent).toBe("Message from another agent · unverified sender");
    expect(summaryOf().textContent).not.toContain("s.unknown.1");
  });

  it("opens the sender's words on the line's click, without a caption", async () => {
    await renderCard(message("s.local.1", { kind: "local" }), names(new Map()));
    expect(container.querySelector("blockquote")).toBeNull();
    const details = container.querySelector("details");
    if (details === null) throw new Error("the line had nothing to open");
    await act(async () => {
      details.open = true;
      details.dispatchEvent(new Event("toggle"));
    });
    expect(container.querySelector(".workspace-chat-activity-body")?.textContent).toBe(
      "hello from the other device",
    );
    expect(container.querySelector("figcaption")).toBeNull();
  });
});
