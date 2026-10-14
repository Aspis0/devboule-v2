// @vitest-environment happy-dom

// The grant door of a consent card: one option is a plain button, several
// collapse under "Allow" with each option named by the daemon. A chooser is
// never one of them — it says its own words, option by option.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PermissionRequest } from "../types/ipc";

const mocks = vi.hoisted(() => ({
  sessionPermissionRespond: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({
  sessionPermissionRespond: mocks.sessionPermissionRespond,
}));

import { PermissionCard } from "./PermissionCard";

/** The ordinary pair: one way in, one way out. */
const oneAllow: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-a",
  title: "Run command",
  options: [
    { optionId: "allow", name: "Just this once", kind: "allow_once" },
    { optionId: "deny", name: "Deny", kind: "reject_once" },
  ],
};

/** A consent card that offers three scopes to grant. */
const threeScopes: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-b",
  title: "Run command",
  options: [
    { optionId: "once", name: "Just this once", kind: "allow_once" },
    { optionId: "session", name: "For this session", kind: "allow_session" },
    { optionId: "always", name: "Always", kind: "allow_always" },
    { optionId: "reject", name: "Deny", kind: "reject_once" },
  ],
};

/** The agent's own question, whose options are choices and not scopes. */
const chooser: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-c",
  title: "Which colour should the fence be?",
  isChooser: true,
  options: [
    { optionId: "red", name: "Red", kind: "allow_once" },
    { optionId: "green", name: "Green", kind: "allow_once" },
    { optionId: "blue", name: "Blue", kind: "allow_always" },
    { optionId: "none", name: "None", kind: "reject_once" },
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

async function renderCard(request: PermissionRequest) {
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
  return card;
}

function actionButtons(card: Element): HTMLButtonElement[] {
  return Array.from(card.querySelectorAll<HTMLButtonElement>(".permission-card-actions button"));
}

const menuItems = (card: Element) => [
  ...card.querySelectorAll<HTMLButtonElement>("[role=menuitem]"),
];

describe("the grant door of a consent card", () => {
  it("renders one allow option as a plain button, with no chevron and no menu", async () => {
    const card = await renderCard(oneAllow);

    const buttons = actionButtons(card);
    expect(buttons.map((button) => button.textContent)).toEqual(["Deny", "Just this once"]);
    expect(buttons[1].getAttribute("aria-haspopup")).toBeNull();
    expect(card.querySelector(".permission-card-scope-chevron")).toBeNull();
    expect(card.querySelector("[role=menu]")).toBeNull();

    await act(async () => buttons[1].click());
    expect(mocks.sessionPermissionRespond).toHaveBeenCalledWith(
      "session-1",
      41,
      "tool-a",
      "allow_once",
      "allow",
      undefined,
    );
  });

  it("collapses several allow options into one Allow button and lists them by name", async () => {
    const card = await renderCard(threeScopes);

    // The refusal stays a direct control: it is one decision, not a scope.
    const buttons = actionButtons(card);
    expect(buttons.map((button) => button.textContent)).toEqual(["Deny", "Allow"]);
    expect(buttons[1].getAttribute("aria-haspopup")).toBe("menu");
    expect(buttons[1].getAttribute("aria-expanded")).toBe("false");
    expect(card.querySelector(".permission-card-scope-chevron")).not.toBeNull();
    expect(menuItems(card)).toHaveLength(0);

    await act(async () => buttons[1].click());
    expect(buttons[1].getAttribute("aria-expanded")).toBe("true");
    expect(menuItems(card).map((item) => item.textContent)).toEqual([
      "Just this once",
      "For this session",
      "Always",
    ]);
    // Opening hands the keyboard to the list, so the first scope is one Enter away.
    expect(document.activeElement?.textContent).toBe("Just this once");
  });

  it("answers with the option the person picked, and closes the menu", async () => {
    const card = await renderCard(threeScopes);
    const allow = actionButtons(card)[1];
    await act(async () => allow.click());

    await act(async () => menuItems(card)[2].click());

    expect(mocks.sessionPermissionRespond).toHaveBeenCalledWith(
      "session-1",
      41,
      "tool-b",
      "allow_once",
      "always",
      undefined,
    );
    expect(card.querySelector("[role=menu]")).toBeNull();
  });

  it("closes on Escape and gives the key back to the button", async () => {
    const card = await renderCard(threeScopes);
    const allow = actionButtons(card)[1];
    await act(async () => allow.click());

    const menu = card.querySelector("[role=menu]");
    const event = new KeyboardEvent("keydown", {
      key: "Escape",
      bubbles: true,
      cancelable: true,
    });
    await act(async () => menu?.dispatchEvent(event));

    expect(card.querySelector("[role=menu]")).toBeNull();
    expect(document.activeElement).toBe(allow);
    // Escape closed the menu; it did not answer the card.
    expect(mocks.sessionPermissionRespond).not.toHaveBeenCalled();
  });

  it("leaves a chooser with one button per option and no scope menu", async () => {
    const card = await renderCard(chooser);

    expect(actionButtons(card).map((button) => button.textContent)).toEqual([
      "Red",
      "Green",
      "Blue",
      "None",
    ]);
    expect(card.querySelector(".permission-card-scope")).toBeNull();
    expect(card.querySelector("[role=menu]")).toBeNull();
  });
});
