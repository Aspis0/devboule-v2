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
import { optionOutcome } from "../lib/optionOutcome";

/** An ACP question as the daemon delivers it: three options, marked as a chooser. */
const chooserRequest: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-chooser",
  title: "Pick a region",
  isChooser: true,
  options: [
    { optionId: "opt-alpha", name: "Alpha", kind: "allow_once" },
    { optionId: "opt-beta", name: "Beta", kind: "allow_once" },
    { optionId: "opt-gamma", name: "Gamma", kind: "allow_once" },
  ],
};

/** The ordinary pair, exactly what an unmarked request carries. */
const standardRequest: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-standard",
  title: "Run command",
  command: "cmd.exe",
  options: [
    { optionId: "allow", name: "Allow once", kind: "allow_once" },
    { optionId: "deny", name: "Deny", kind: "reject_once" },
  ],
};

/** A chooser whose second option grants for good. */
const durableChooserRequest: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-durable",
  title: "Pick a region",
  isChooser: true,
  options: [
    { optionId: "opt-once", name: "Alpha", kind: "allow_once" },
    { optionId: "opt-always", name: "Beta, always", kind: "allow_always" },
  ],
};

/** The stub's own question: three colours plus the refusal option. */
const rejectOptionChooserRequest: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-chooser",
  title: "Which colour should the fence be?",
  isChooser: true,
  options: [
    { optionId: "red", name: "Red", kind: "allow_once" },
    { optionId: "green", name: "Green", kind: "allow_once" },
    { optionId: "blue", name: "Blue", kind: "allow_once" },
    { optionId: "none", name: "None", kind: "reject_once" },
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
  return { root, card };
}

function actionButtons(card: Element): HTMLButtonElement[] {
  return Array.from(card.querySelectorAll<HTMLButtonElement>(".permission-card-actions button"));
}

function findButton(card: Element, label: string): HTMLButtonElement | undefined {
  return actionButtons(card).find((button) => button.textContent === label);
}

describe("PermissionCard chooser answers", () => {
  beforeEach(() => {
    mocks.sessionPermissionRespond.mockReset();
  });

  afterEach(() => {
    document.body.replaceChildren();
  });

  it("renders three option buttons for a three-option chooser", async () => {
    const { root, card } = await renderCard(chooserRequest);

    // One button per option, in the agent's own order, each under its own
    // name — plus the plain Deny the card keeps when no option refuses.
    expect(actionButtons(card).map((button) => button.textContent)).toEqual([
      "Deny",
      "Alpha",
      "Beta",
      "Gamma",
    ]);

    await act(async () => root.unmount());
  });

  it("clicking the second option posts its optionId", async () => {
    const { root, card } = await renderCard(chooserRequest);
    const beta = findButton(card, "Beta");
    if (beta === undefined) throw new Error("the second option button did not render");

    await act(async () => beta.click());

    expect(mocks.sessionPermissionRespond).toHaveBeenCalledWith(
      "session-1",
      41,
      "tool-chooser",
      "allow_once",
      "opt-beta",
      undefined,
    );

    await act(async () => root.unmount());
  });

  it("names the chosen option once this card has answered", async () => {
    const { root, card } = await renderCard(chooserRequest);
    const beta = findButton(card, "Beta");
    if (beta === undefined) throw new Error("the second option button did not render");

    await act(async () => beta.click());

    expect(card.querySelector(".permission-card-choice")?.textContent).toBe("Chosen: Beta");

    await act(async () => root.unmount());
  });

  it("a standard request still renders Allow once and Deny", async () => {
    const { root, card } = await renderCard(standardRequest);

    expect(actionButtons(card).map((button) => button.textContent)).toEqual(["Deny", "Allow once"]);

    await act(async () => root.unmount());
  });

  it("a durable choice reads as allowed always and names the option picked", async () => {
    const { root, card } = await renderCard(durableChooserRequest);
    const always = findButton(card, "Beta, always");
    if (always === undefined) throw new Error("the durable option button did not render");

    await act(async () => always.click());

    // The journal's rule is the card's rule: the granted kind is what the
    // resolved state says, never a one-shot constant.
    expect(card.querySelector(".permission-card-label")?.textContent).toBe(
      "Allowed always \u00b7 running",
    );
    expect(card.querySelector(".permission-card-choice")?.textContent).toBe("Chosen: Beta, always");

    await act(async () => root.unmount());
  });

  it("Deny on a chooser with no reject option resolves as Denied, with no error", async () => {
    const { root, card } = await renderCard(chooserRequest);
    const deny = findButton(card, "Deny");
    if (deny === undefined) throw new Error("the kept Deny button did not render");

    await act(async () => deny.click());

    // ACP's only refusal for this request is the cancellation the daemon
    // delivers — an answer, so the card reads Denied: not an error slot,
    // not back to waiting.
    expect(card.querySelector(".permission-card-label")?.textContent).toBe(
      "Denied \u2014 the turn continues without it",
    );
    expect(card.querySelector("[role=alert]")).toBeNull();

    await act(async () => root.unmount());
  });

  it("a chooser whose options include a refusal renders them all and its reject option resolves as Denied", async () => {
    const { root, card } = await renderCard(rejectOptionChooserRequest);

    // The refusal rides in as one of the agent's options, so the card keeps
    // no plain Deny beside them: the option IS the refusal (review A2a #12
    // found this branch untested).
    expect(actionButtons(card).map((button) => button.textContent)).toEqual([
      "Red",
      "Green",
      "Blue",
      "None",
    ]);
    const none = findButton(card, "None");
    if (none === undefined) throw new Error("the reject option button did not render");

    await act(async () => none.click());

    expect(card.querySelector(".permission-card-label")?.textContent).toBe(
      "Denied \u2014 the turn continues without it",
    );
    expect(card.querySelector("[role=alert]")).toBeNull();
    expect(card.querySelector(".permission-card-choice")?.textContent).toBe("Chosen: None");

    await act(async () => root.unmount());
  });
});

describe("optionOutcome", () => {
  it("sends a reject* option to deny and any other kind to allow_once", () => {
    // The brief's mapping rule: allow* posts allow_once, reject* posts deny.
    expect(optionOutcome("allow_once")).toBe("allow_once");
    expect(optionOutcome("allow_always")).toBe("allow_once");
    expect(optionOutcome("reject_once")).toBe("deny");
    expect(optionOutcome("reject_always")).toBe("deny");
  });

  it("never claims a grant for a kind it does not know", () => {
    // Fail closed: an unknown kind is never posted, or styled, as a grant —
    // the daemon validates the pairing either way.
    expect(optionOutcome("info")).toBe("deny");
    expect(optionOutcome("editor")).toBe("deny");
  });
});
