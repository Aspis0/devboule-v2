// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PermissionRequest } from "../types/ipc";

const mocks = vi.hoisted(() => ({
  sessionPermissionRespond: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({
  sessionPermissionRespond: mocks.sessionPermissionRespond,
}));

import { PermissionCard } from "./PermissionCard";

/** A Claude question as the daemon delivers it: one item, two options, no chooser mark needed. */
const singleRequest: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-question",
  title: "Which colour should I paint the fence?",
  description: "Forest green (Recommended) / Barn red",
  kind: "question",
  options: [
    { optionId: "q0o0", name: "Forest green (Recommended)", kind: "allow_once" },
    { optionId: "q0o1", name: "Barn red", kind: "allow_once" },
  ],
  questions: [
    {
      question: "Which colour should I paint the fence?",
      options: [
        { label: "Forest green (Recommended)", description: "Blends in." },
        { label: "Barn red", description: "Classic." },
      ],
      multiSelect: false,
    },
  ],
};

/** One question whose options combine: checkboxes, joined on Submit. */
const multiSelectRequest: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-multi",
  title: "Which toppings?",
  kind: "question",
  options: [
    { optionId: "q0o0", name: "Cheese", kind: "allow_once" },
    { optionId: "q0o1", name: "Pepperoni", kind: "allow_once" },
  ],
  questions: [
    {
      question: "Which toppings?",
      options: [{ label: "Cheese" }, { label: "Pepperoni" }],
      multiSelect: true,
    },
  ],
};

async function renderCard(request: PermissionRequest) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      <PermissionCard
        sessionId="session-1"
        subscriptionId={41}
        request={request}
        capabilities={["typed_permissions"]}
      />,
    );
  });
  const card = container.querySelector(".permission-card");
  if (card === null) throw new Error("permission card did not render");
  return { root, card, container };
}

function actions(card: Element): HTMLButtonElement[] {
  return Array.from(card.querySelectorAll<HTMLButtonElement>(".permission-card-actions button"));
}

function findButton(card: Element, label: string): HTMLButtonElement | undefined {
  return actions(card).find((button) => button.textContent === label);
}

function radios(card: Element): HTMLInputElement[] {
  return Array.from(card.querySelectorAll<HTMLInputElement>('input[type="radio"]'));
}

function checkboxes(card: Element): HTMLInputElement[] {
  return Array.from(card.querySelectorAll<HTMLInputElement>('input[type="checkbox"]'));
}

function otherInput(card: Element): HTMLInputElement | null {
  return card.querySelector<HTMLInputElement>('.permission-card-question-other input[type="text"]');
}

describe("PermissionCard question answers", () => {
  beforeEach(() => {
    mocks.sessionPermissionRespond.mockReset();
    mocks.sessionPermissionRespond.mockResolvedValue(undefined);
  });

  afterEach(() => {
    document.body.replaceChildren();
  });

  it("renders the question, radio options with descriptions, Other and Submit", async () => {
    const { root, card } = await renderCard(singleRequest);

    expect(card.querySelector(".permission-card-question-text")?.textContent).toBe(
      "Which colour should I paint the fence?",
    );
    expect(radios(card)).toHaveLength(2);
    expect(card.textContent).toContain("Blends in.");
    expect(otherInput(card)).not.toBeNull();
    expect(actions(card).map((button) => button.textContent)).toEqual(["Dismiss", "Submit"]);

    await act(async () => root.unmount());
  });

  it("Submit stays disabled until the question is answered", async () => {
    const { root, card } = await renderCard(singleRequest);
    const submit = findButton(card, "Submit");
    if (submit === undefined) throw new Error("Submit did not render");

    expect(submit.disabled).toBe(true);

    await act(async () => radios(card)[0].click());

    expect(findButton(card, "Submit")?.disabled).toBe(false);

    await act(async () => root.unmount());
  });

  it("a lone single-select pick posts its optionId", async () => {
    const { root, card, container } = await renderCard(singleRequest);

    await act(async () => radios(card)[1].click());
    const submit = findButton(card, "Submit");
    if (submit === undefined) throw new Error("Submit did not render");
    await act(async () => submit.click());

    expect(mocks.sessionPermissionRespond).toHaveBeenCalledWith(
      "session-1",
      41,
      "tool-question",
      "allow_once",
      "q0o1",
      undefined,
    );
    expect(container.querySelector(".permission-card-choice")?.textContent).toBe(
      "Chosen: Barn red",
    );

    await act(async () => root.unmount());
  });

  it("typed Other text posts the answer door, not an optionId", async () => {
    const { root, card } = await renderCard(singleRequest);
    const other = otherInput(card);
    if (other === null) throw new Error("Other field did not render");

    // No testing-library on this repo: drive React's onChange through the
    // native value setter plus a bubbling input event.
    const nativeSetter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      "value",
    )?.set;
    await act(async () => {
      nativeSetter?.call(other, "Teal, obviously");
      other.dispatchEvent(new Event("input", { bubbles: true }));
    });
    const submit = findButton(card, "Submit");
    if (submit === undefined) throw new Error("Submit did not render");
    await act(async () => submit.click());

    expect(mocks.sessionPermissionRespond).toHaveBeenCalledWith(
      "session-1",
      41,
      "tool-question",
      "allow_once",
      undefined,
      "Teal, obviously",
    );

    await act(async () => root.unmount());
  });

  it("multi-select renders checkboxes and posts the joined labels as the answer", async () => {
    const { root, card } = await renderCard(multiSelectRequest);

    expect(radios(card)).toHaveLength(0);
    expect(checkboxes(card)).toHaveLength(2);

    await act(async () => checkboxes(card)[0].click());
    await act(async () => checkboxes(card)[1].click());
    const submit = findButton(card, "Submit");
    if (submit === undefined) throw new Error("Submit did not render");
    await act(async () => submit.click());

    expect(mocks.sessionPermissionRespond).toHaveBeenCalledWith(
      "session-1",
      41,
      "tool-multi",
      "allow_once",
      undefined,
      "Cheese, Pepperoni",
    );

    await act(async () => root.unmount());
  });

  it("Dismiss refuses without naming an option", async () => {
    const { root, card } = await renderCard(singleRequest);
    const dismiss = findButton(card, "Dismiss");
    if (dismiss === undefined) throw new Error("Dismiss did not render");

    await act(async () => dismiss.click());

    expect(mocks.sessionPermissionRespond).toHaveBeenCalledWith(
      "session-1",
      41,
      "tool-question",
      "deny",
    );

    await act(async () => root.unmount());
  });
});
