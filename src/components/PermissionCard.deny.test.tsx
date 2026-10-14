// @vitest-environment happy-dom

// The card's imperative refusal, for the pane's Escape handler. It is the same
// answer its refusal button makes, so it runs that button's gates: a card the
// daemon cannot take, one already answered or gone stale, one mid-answer, and
// one whose own controls say the refusal is not on offer.

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

function withOptions(options: PermissionRequest["options"], toolCallId = "tool-a") {
  return { ...request, toolCallId, options };
}

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

async function renderCard(props: {
  card: React.RefObject<PermissionCardHandle | null>;
  request?: PermissionRequest;
  daemonState?: "connected" | "disconnected";
}) {
  await act(async () => {
    root.render(
      <PermissionCard
        ref={props.card}
        sessionId="session-1"
        subscriptionId={41}
        request={props.request ?? request}
        capabilities={["typed_permissions"]}
        daemonState={props.daemonState ?? "connected"}
      />,
    );
  });
}

describe("a card refused from outside", () => {
  it("answers deny on the card's own behalf", async () => {
    const card = createRef<PermissionCardHandle>();
    await renderCard({ card });

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

  it("refuses nothing on a request that offered no reject option", async () => {
    const card = createRef<PermissionCardHandle>();
    await renderCard({
      card,
      request: withOptions([{ optionId: "allow", name: "Allow once", kind: "allow_once" }]),
    });
    const deny = container.querySelector<HTMLButtonElement>(".permission-card-deny-action");
    expect(deny?.disabled).toBe(true);

    await act(async () => card.current?.deny());

    // The daemon cancels a request it has no reject option for, so the key must
    // not reach an answer the card itself declines to offer.
    expect(mocks.sessionPermissionRespond).not.toHaveBeenCalled();
  });

  it("refuses nothing when only a durable refusal was offered", async () => {
    const card = createRef<PermissionCardHandle>();
    await renderCard({
      card,
      request: withOptions(
        [
          { optionId: "allow", name: "Allow once", kind: "allow_once" },
          { optionId: "never", name: "Never", kind: "reject_always" },
        ],
        "tool-durable",
      ),
    });
    expect(
      container.querySelector<HTMLButtonElement>(".permission-card-deny-action")?.disabled,
    ).toBe(true);

    await act(async () => card.current?.deny());

    // A bare deny cannot resolve a durable refusal; the daemon would keep the
    // request pending and hand the card an error it asked for.
    expect(mocks.sessionPermissionRespond).not.toHaveBeenCalled();
  });

  it("refuses nothing while the daemon is unreachable", async () => {
    const card = createRef<PermissionCardHandle>();
    await renderCard({ card, daemonState: "disconnected" });

    await act(async () => card.current?.deny());

    expect(mocks.sessionPermissionRespond).not.toHaveBeenCalled();
    expect(container.querySelector(".permission-card-label")?.textContent).toBe("Waiting on you");
  });

  it("refuses a chooser only where its own Deny control renders", async () => {
    const card = createRef<PermissionCardHandle>();
    await renderCard({
      card,
      request: {
        ...request,
        toolCallId: "tool-chooser",
        isChooser: true,
        options: [
          { optionId: "red", name: "Red", kind: "allow_once" },
          { optionId: "none", name: "None", kind: "reject_once" },
        ],
      },
    });
    // The agent offered its own refusal, so the card renders no bare Deny of
    // its own — only the options it named.
    expect(
      [...container.querySelectorAll<HTMLButtonElement>(".permission-card-actions button")].map(
        (button) => button.textContent,
      ),
    ).toEqual(["Red", "None"]);

    await act(async () => card.current?.deny());

    expect(mocks.sessionPermissionRespond).not.toHaveBeenCalled();
  });

  it("refuses a question, whose Dismiss is the card's own bare denial", async () => {
    const card = createRef<PermissionCardHandle>();
    await renderCard({
      card,
      request: {
        type: "permission_request",
        toolCallId: "tool-q",
        title: "How many?",
        kind: "question",
        questions: [{ question: "How many?", multiSelect: false, options: [{ label: "Three" }] }],
        options: [{ optionId: "none", name: "None", kind: "reject_once" }],
      },
    });

    await act(async () => card.current?.deny());

    expect(mocks.sessionPermissionRespond).toHaveBeenCalledWith(
      "session-1",
      41,
      "tool-q",
      "deny",
      undefined,
      undefined,
    );
  });

  it("refuses nothing twice: a second call lands on an answered card", async () => {
    const card = createRef<PermissionCardHandle>();
    await renderCard({ card });

    await act(async () => card.current?.deny());
    await act(async () => card.current?.deny());

    expect(mocks.sessionPermissionRespond).toHaveBeenCalledTimes(1);
  });

  it("refuses nothing while an answer is still in flight", async () => {
    let resolveRespond!: (value: undefined) => void;
    mocks.sessionPermissionRespond.mockReturnValueOnce(
      new Promise<undefined>((resolve) => {
        resolveRespond = resolve;
      }),
    );
    const card = createRef<PermissionCardHandle>();
    await renderCard({ card });

    await act(async () => card.current?.deny());
    await act(async () => card.current?.deny());

    expect(mocks.sessionPermissionRespond).toHaveBeenCalledTimes(1);
    expect(container.querySelector(".permission-card-label")?.textContent).toBe(
      "Sending decision…",
    );

    await act(async () => {
      resolveRespond(undefined);
    });
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

  it("refuses nothing on a stale card, which the daemon no longer knows", async () => {
    const card = createRef<PermissionCardHandle>();
    await act(async () => {
      root.render(
        <PermissionCard
          ref={card}
          sessionId="session-1"
          subscriptionId={41}
          request={request}
          capabilities={["typed_permissions"]}
          stale
        />,
      );
    });

    await act(async () => card.current?.deny());

    expect(mocks.sessionPermissionRespond).not.toHaveBeenCalled();
  });
});
