// @vitest-environment happy-dom

// A card whose answer the daemon can no longer take must stop offering the
// same answer again: the terminal state says so and keeps only Clear.

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

const request: PermissionRequest = {
  type: "permission_request",
  toolCallId: "tool-a",
  title: "Run command",
  command: "cmd.exe",
  args: ["/c", "echo", "alpha"],
  cwd: "C:\\alpha",
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

async function renderCard(onResolved?: (sessionId: string, toolCallId: string) => void) {
  await act(async () => {
    root.render(
      <PermissionCard
        sessionId="session-1"
        subscriptionId={41}
        request={request}
        capabilities={["typed_permissions"]}
        onResolved={onResolved}
      />,
    );
  });
}

function label(): string {
  return container.querySelector(".permission-card-label")?.textContent ?? "";
}

async function answer() {
  await act(async () => {
    container.querySelector<HTMLButtonElement>(".permission-card-primary-action")?.click();
    await Promise.resolve();
  });
}

describe("stale permission answer", () => {
  it("a no-longer-pending refusal ends the card: the line, Clear, no retry", async () => {
    const onResolved = vi.fn();
    mocks.sessionPermissionRespond.mockRejectedValueOnce({
      code: "invalid_request",
      message: "permission request is no longer pending",
    });
    await renderCard(onResolved);

    await answer();

    expect(label()).toBe("This request is no longer pending.");
    const clear = container.querySelector<HTMLButtonElement>(".permission-card-dismiss-action");
    if (clear === null) throw new Error("clear control did not render on the stale card");
    expect(container.querySelector(".permission-card-primary-action")).toBeNull();
    expect(container.querySelector(".permission-card-deny-action")).toBeNull();
    await act(async () => clear.click());
    expect(onResolved).toHaveBeenCalledWith("session-1", "tool-a");
  });

  it("a missing-broker refusal ends the card the same way", async () => {
    mocks.sessionPermissionRespond.mockRejectedValueOnce({
      code: "invalid_request",
      message: "Session has no live ACP permission broker.",
    });
    await renderCard();

    await answer();

    expect(label()).toBe("This request is no longer pending.");
    expect(container.querySelector(".permission-card-dismiss-action")).not.toBeNull();
    expect(container.querySelector(".permission-card-primary-action")).toBeNull();
  });

  it("a bad option id stays answerable — same code, retryable card", async () => {
    mocks.sessionPermissionRespond.mockRejectedValueOnce({
      code: "invalid_request",
      message: "Unknown permission option 'allow'.",
    });
    await renderCard();

    await answer();

    expect(label()).toBe("Waiting on you");
    const allow = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
    if (allow === null) throw new Error("allow control did not render after a bad-option error");
    expect(allow.disabled).toBe(false);
    expect(container.querySelector("[role=alert]")?.textContent).toContain(
      "Unknown permission option",
    );
  });

  it("any other refusal stays answerable", async () => {
    mocks.sessionPermissionRespond.mockRejectedValueOnce(new Error("live failed"));
    await renderCard();

    await answer();

    expect(label()).toBe("Waiting on you");
    expect(
      container.querySelector<HTMLButtonElement>(".permission-card-primary-action")?.disabled,
    ).toBe(false);
  });
});

describe("stale permission answer, second pass", () => {
  it("a gone session ends the card the same way", async () => {
    mocks.sessionPermissionRespond.mockRejectedValueOnce({
      code: "session_not_found",
      message: "No session with that id.",
    });
    await renderCard();

    await answer();

    expect(label()).toBe("This request is no longer pending.");
    expect(container.querySelector(".permission-card-dismiss-action")).not.toBeNull();
    expect(container.querySelector(".permission-card-primary-action")).toBeNull();
  });

  it("the terminal card announces once: no alert next to the label", async () => {
    mocks.sessionPermissionRespond.mockRejectedValueOnce({
      code: "invalid_request",
      message: "permission request is no longer pending",
    });
    await renderCard();

    await answer();

    expect(label()).toBe("This request is no longer pending.");
    expect(container.querySelector("[role=alert]")).toBeNull();
  });

  it("Clear on a stale card does not claim anything was answered", async () => {
    mocks.sessionPermissionRespond.mockRejectedValueOnce({
      code: "invalid_request",
      message: "permission request is no longer pending",
    });
    await renderCard();

    await answer();

    const clear = container.querySelector<HTMLButtonElement>(".permission-card-dismiss-action");
    if (clear === null) throw new Error("clear control did not render on the stale card");
    expect(clear.getAttribute("aria-label")).toBe("Clear this request");
  });

  it("a question card goes terminal through Dismiss", async () => {
    const questionRequest: PermissionRequest = {
      ...request,
      toolCallId: "tool-question",
      title: "Which colour should I paint the fence?",
      kind: "question",
      options: [
        { optionId: "q0o0", name: "Forest green (Recommended)", kind: "allow_once" },
        { optionId: "q0o1", name: "Barn red", kind: "allow_once" },
      ],
      questions: [
        {
          question: "Which colour should I paint the fence?",
          options: [{ label: "Forest green (Recommended)" }, { label: "Barn red" }],
          multiSelect: false,
        },
      ],
    };
    mocks.sessionPermissionRespond.mockRejectedValueOnce({
      code: "invalid_request",
      message: "Session has no live ACP permission broker.",
    });
    await act(async () => {
      root.render(
        <PermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={questionRequest}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    await act(async () => {
      container.querySelector<HTMLButtonElement>(".permission-card-deny-action")?.click();
      await Promise.resolve();
    });

    expect(label()).toBe("This request is no longer pending.");
    expect(container.querySelector(".permission-card-dismiss-action")).not.toBeNull();
    expect(container.querySelector(".permission-card-questions")).toBeNull();
  });
});
