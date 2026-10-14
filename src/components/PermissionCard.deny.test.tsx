// @vitest-environment happy-dom

// The card's imperative refusal, for the pane's Escape handler. It is the same
// answer its Deny button makes, so the same gates apply: a card that has
// already answered, gone stale, or is mid-answer refuses nothing.

import { act, createRef } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PermissionRequest } from "../types/ipc";

const mocks = vi.hoisted(() => ({
  sessionPermissionRespond: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({
  sessionPermissionRespond: mocks.sessionPermissionRespond,
}));

import { PermissionCard, type PermissionCardHandle } from "./PermissionCard";

const request: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-a",
  title: "Run command",
  options: [
    { optionId: "allow", name: "Allow once", kind: "allow_once" },
    { optionId: "deny", name: "Deny", kind: "reject_once" },
  ],
};

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  mocks.sessionPermissionRespond.mockReset();
  mocks.sessionPermissionRespond.mockResolvedValue(undefined);
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

async function renderCard(card: React.RefObject<PermissionCardHandle | null>) {
  await act(async () => {
    root.render(
      <PermissionCard
        ref={card}
        sessionId="session-1"
        subscriptionId={41}
        request={request}
        capabilities={["typed_permissions"]}
      />,
    );
  });
}

describe("a card refused from outside", () => {
  it("answers deny on the card's own behalf, gates included", async () => {
    const card = createRef<PermissionCardHandle>();
    await renderCard(card);

    await act(async () => card.current?.deny());

    expect(mocks.sessionPermissionRespond).toHaveBeenCalledWith(
      "session-1",
      41,
      "tool-a",
      "deny",
      undefined,
      undefined,
    );
    expect(container.querySelector(".permission-card-label")?.textContent).toBe(
      "Denied — the turn continues without it",
    );
  });

  it("refuses nothing twice: a second call lands on an answered card", async () => {
    const card = createRef<PermissionCardHandle>();
    await renderCard(card);

    await act(async () => card.current?.deny());
    await act(async () => card.current?.deny());

    expect(mocks.sessionPermissionRespond).toHaveBeenCalledTimes(1);
  });

  it("refuses nothing on a card the daemon has already answered", async () => {
    const card = createRef<PermissionCardHandle>();
    await act(async () => {
      root.render(
        <PermissionCard
          ref={card}
          sessionId="session-1"
          subscriptionId={41}
          request={request}
          capabilities={["typed_permissions"]}
          resolution={{
            outcome: "allowed",
            answeredBy: "s.parent",
            selectedOptionName: "Allow once",
          }}
        />,
      );
    });

    await act(async () => card.current?.deny());

    expect(mocks.sessionPermissionRespond).not.toHaveBeenCalled();
  });
});
