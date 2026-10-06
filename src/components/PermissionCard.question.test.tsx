// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { assembleCssProof, removeCssProof } from "../features/workspace/cssProof";
import type { PermissionRequest } from "../types/ipc";

const mocks = vi.hoisted(() => ({
  sessionPermissionRespond: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({
  sessionPermissionRespond: mocks.sessionPermissionRespond,
}));

import { PermissionCard } from "./PermissionCard";

const rootDir = resolve(import.meta.dirname, "../..");
const read = (path: string) => readFileSync(resolve(rootDir, path), "utf8");
const cardCss = assembleCssProof([
  read("src/styles/tokens.css"),
  read("src/components/PermissionCard.css"),
]);

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
  return card.querySelector<HTMLInputElement>(".permission-card-question-other input");
}

describe("PermissionCard question answers", () => {
  beforeEach(() => {
    mocks.sessionPermissionRespond.mockReset();
    mocks.sessionPermissionRespond.mockResolvedValue(undefined);
  });

  afterEach(() => {
    document.body.replaceChildren();
    removeCssProof();
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

  it("a chosen chip exposes its checked state and the check icon", async () => {
    const { root, card } = await renderCard(singleRequest);
    const firstRadio = radios(card)[0];
    if (firstRadio === undefined) throw new Error("radio option did not render");
    const chip = firstRadio.closest(".permission-card-question-option");
    if (chip === null) throw new Error("option chip did not render");

    // Unchosen: no check on the chip, no chosen mark.
    expect(chip.querySelector(".permission-card-check")).toBeNull();
    expect(chip.className).not.toContain("permission-card-question-option-chosen");

    await act(async () => firstRadio.click());

    // Chosen: the input's checked is the exposed state, and the check icon
    // is on the chip.
    expect(firstRadio.checked).toBe(true);
    expect(chip.className).toContain("permission-card-question-option-chosen");
    expect(chip.querySelector(".permission-card-check")).not.toBeNull();

    await act(async () => root.unmount());
  });

  it("lets a long option description wrap inside its chip", async () => {
    const { root, card } = await renderCard({
      ...singleRequest,
      toolCallId: "tool-long",
      questions: [
        {
          question: "Which colour should I paint the fence?",
          options: [
            {
              label: "Forest green",
              description: "A deep, calm green that blends into the treeline behind the garden.",
            },
          ],
          multiSelect: false,
        },
      ],
    });
    const chip = card.querySelector(".permission-card-question-option");
    const description = card.querySelector(".permission-card-question-description");
    if (chip === null || description === null) throw new Error("option chip did not render");

    // The chip's geometry is min-height 26 with no fixed height (pinned in
    // PermissionCard.computed.test.tsx), so a wrapped description grows the
    // chip instead of rendering over its border.
    cardCss.inject([".permission-card-question-option"]);
    expect(getComputedStyle(chip).minHeight).toBe("26px");
    // And the description is the chip's own content, never a line over it.
    expect(chip.contains(description)).toBe(true);
    expect(description.textContent).toBe(
      "A deep, calm green that blends into the treeline behind the garden.",
    );

    await act(async () => root.unmount());
  });

  it("keeps a question request's command and working directory on the card", async () => {
    const { root, card } = await renderCard({
      ...singleRequest,
      toolCallId: "tool-question-command",
      command: "pnpm db migrate --target postgres16",
      cwd: "C:\\alpha",
    });

    // A question request can carry a command: it renders inside the card, on
    // its own ground.
    const command = card.querySelector(".permission-card-command");
    if (command === null) throw new Error("command line did not render");
    expect(card.contains(command)).toBe(true);
    expect(command.textContent).toBe("pnpm db migrate --target postgres16");
    // The question card has no head — the spec gives it none, and its help
    // icon is its marker. Its working directory is a meta line in the actions
    // row instead (fix pass 2, review N2).
    expect(card.querySelector(".permission-card-heading")).toBeNull();
    expect(card.textContent).not.toContain("Permission");
    const cwd = card.querySelector<HTMLElement>(".permission-card-cwd");
    if (cwd === null) throw new Error("working directory line did not render");
    expect(cwd.textContent).toBe("C:\\alpha");
    expect(cwd.title).toBe("C:\\alpha");

    await act(async () => root.unmount());
  });

  it("gives the Other field the same focus ring as the option chips", async () => {
    const { root, card } = await renderCard(singleRequest);
    const other = card.querySelector(".permission-card-question-other");
    if (other === null) throw new Error("Other door did not render");
    // The Other door is a chip: it carries the class the ring rule is keyed on.
    expect(other.className).toContain("permission-card-question-option");

    // The ring rule's descendant test is the bare :focus-visible — the form
    // that reaches every input in the row, the Other door's included. The
    // N1 regression scoped it to .permission-card-question-input, a class the
    // Other input does not carry; this assertion fails if that comes back.
    // happy-dom matches every :has(), so this pins the rule's shape, not its
    // match — only a live check proves the Other input triggers the ring
    // (noted in the report).
    const ring = cardCss.rulesFor(".permission-card-question-option:has(:focus-visible)");
    expect(ring).toContain("outline: 2px solid #7a5000");
    expect(ring).toContain("outline-offset: 2px");

    await act(async () => root.unmount());
  });

  it("scopes radio groups to the card so two cards never share one", async () => {
    const { root, card } = await renderCard(singleRequest);

    // Provider-chosen ids can repeat across sessions; the group name carries
    // the session, subscription and tool call so a second card cannot clear
    // this one's pick.
    for (const radio of radios(card)) {
      expect(radio.name).toBe("permission-question-session-1-41-tool-question-0");
    }

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

  it("a rejected Submit shows the error with no Chosen line", async () => {
    const { root, card } = await renderCard(singleRequest);

    // The answer door: a lone pick would travel as an option id and name
    // itself inside `respond`, so only typed text reaches the fixed line.
    const other = otherInput(card);
    if (other === null) throw new Error("Other field did not render");
    const nativeSetter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      "value",
    )?.set;
    await act(async () => {
      nativeSetter?.call(other, "Teal, obviously");
      other.dispatchEvent(new Event("input", { bubbles: true }));
    });
    mocks.sessionPermissionRespond.mockRejectedValueOnce(new Error("daemon gone"));
    const submit = findButton(card, "Submit");
    if (submit === undefined) throw new Error("Submit did not render");
    await act(async () => submit.click());

    expect(card.querySelector("[role=alert]")).not.toBeNull();
    expect(card.querySelector(".permission-card-choice")).toBeNull();

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
      undefined,
      undefined,
    );

    await act(async () => root.unmount());
  });

  it("hides Other when the question disallows it, and a pick still answers", async () => {
    const { root, card } = await renderCard({
      ...singleRequest,
      toolCallId: "tool-no-other",
      questions: [
        {
          question: "Which colour should I paint the fence?",
          options: [{ label: "Forest green" }, { label: "Barn red" }],
          multiSelect: false,
          allowOther: false,
        },
      ],
    });

    // No free-text door: an approval-as-question carrier's options are the
    // only answers, so an unrecognized typed label can never reach Codex.
    expect(otherInput(card)).toBeNull();
    expect(findButton(card, "Submit")?.disabled).toBe(true);

    await act(async () => radios(card)[0].click());
    expect(findButton(card, "Submit")?.disabled).toBe(false);
    const submit = findButton(card, "Submit");
    if (submit === undefined) throw new Error("Submit did not render");
    await act(async () => submit.click());

    expect(mocks.sessionPermissionRespond).toHaveBeenCalledWith(
      "session-1",
      41,
      "tool-no-other",
      "allow_once",
      "q0o0",
      undefined,
    );

    await act(async () => root.unmount());
  });

  it("masks a secret answer and never echoes it back", async () => {
    const { root, card } = await renderCard({
      ...singleRequest,
      toolCallId: "tool-secret",
      options: [],
      questions: [
        {
          question: "What is the deploy token?",
          options: [],
          multiSelect: false,
          secret: true,
        },
      ],
    });

    const other = otherInput(card);
    if (other === null) throw new Error("Other field did not render");
    // Option-less questions always offer text, secret or not.
    expect(other.type).toBe("password");
    const nativeSetter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      "value",
    )?.set;
    await act(async () => {
      nativeSetter?.call(other, "hunter2");
      other.dispatchEvent(new Event("input", { bubbles: true }));
    });
    const submit = findButton(card, "Submit");
    if (submit === undefined) throw new Error("Submit did not render");
    await act(async () => submit.click());

    expect(mocks.sessionPermissionRespond).toHaveBeenCalledWith(
      "session-1",
      41,
      "tool-secret",
      "allow_once",
      undefined,
      "hunter2",
    );
    // The words travel to the provider that asked, and nowhere on screen.
    expect(card.querySelector(".permission-card-choice")).toBeNull();

    await act(async () => root.unmount());
  });
});
