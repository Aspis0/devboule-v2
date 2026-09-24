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
});

describe("optionOutcome", () => {
  it("sends a reject* option to deny and any other kind to allow_once", () => {
    // The brief's mapping rule: allow* posts allow_once, reject* posts deny.
    expect(optionOutcome("allow_once")).toBe("allow_once");
    expect(optionOutcome("allow_always")).toBe("allow_once");
    expect(optionOutcome("reject_once")).toBe("deny");
    expect(optionOutcome("reject_always")).toBe("deny");
  });
});
