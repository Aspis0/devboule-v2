// The card's sender label is the boundary between authenticated provenance
// and the local roster: far labels stay raw, while local ids may resolve.
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

    const copy = container.querySelector<HTMLElement>(".workspace-chat-copy");
    expect(copy?.textContent).toBe("Message from s.msg.source — device Marco's phone.");
    expect(copy?.textContent).not.toContain("Local look-alike");
    expect(copy?.getAttribute("title")).toBe(`${fromAgent} · ${DEVICE_ID}`);
  });

  it("shows a peer label whole when its device disagrees with the origin", async () => {
    const fromAgent = `peer:${OTHER_DEVICE_ID}/s.msg.source`;
    await renderCard(
      message(fromAgent, { kind: "peer", device: DEVICE_ID }),
      names(new Map([[fromAgent, { id: fromAgent, title: "Forged local name", kind: "claude" }]])),
    );

    const copy = container.querySelector<HTMLElement>(".workspace-chat-copy");
    expect(copy?.textContent).toBe(`Message from ${fromAgent} — device Marco's phone.`);
    expect(copy?.textContent).not.toContain("Forged local name");
  });

  it("still resolves a local sender against the roster", async () => {
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

    expect(container.querySelector(".workspace-chat-copy")?.textContent).toBe(
      "Message from Local worker — this machine.",
    );
  });
});
