// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PermissionRequest } from "../types/ipc";

const mocks = vi.hoisted(() => ({
  sessionPermissionRespond: vi.fn(),
  reasonFromCause: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({
  sessionPermissionRespond: mocks.sessionPermissionRespond,
  reasonFromCause: mocks.reasonFromCause,
}));

import { PermissionCard } from "./PermissionCard";

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

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

describe("PermissionCard", () => {
  beforeEach(() => {
    mocks.sessionPermissionRespond.mockReset();
    mocks.reasonFromCause.mockReset();
    mocks.reasonFromCause.mockImplementation((cause: unknown) =>
      cause instanceof Error ? cause.message : String(cause),
    );
  });

  afterEach(() => {
    document.body.replaceChildren();
  });

  it("re-enables the card and answers a re-delivered request with its new subscription", async () => {
    const first = deferred<void>();
    mocks.sessionPermissionRespond.mockReturnValueOnce(first.promise);

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const allowButton = () => {
      const button = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
      if (button === null) throw new Error("allow control did not render");
      return button;
    };
    const label = () => container.querySelector(".permission-card-label")?.textContent ?? "";

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

    await act(async () => allowButton().click());
    expect(mocks.sessionPermissionRespond).toHaveBeenCalledWith(
      "session-1",
      41,
      "tool-a",
      "allow_once",
    );
    expect(allowButton().disabled).toBe(true);
    expect(label()).toBe("Sending decision…");

    // The daemon re-delivers the same (sessionId, toolCallId) after a re-attach
    // and the host adopts the new subscription id.
    await act(async () => {
      root.render(
        <PermissionCard
          sessionId="session-1"
          subscriptionId={42}
          request={request}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    expect(allowButton().disabled).toBe(false);
    expect(label()).toBe("Waiting on you");

    await act(async () => allowButton().click());
    expect(mocks.sessionPermissionRespond).toHaveBeenCalledTimes(2);
    expect(mocks.sessionPermissionRespond).toHaveBeenLastCalledWith(
      "session-1",
      42,
      "tool-a",
      "allow_once",
    );

    await act(async () => root.unmount());
  });

  it("ignores an abandoned answer that resolves after the subscription changed", async () => {
    const first = deferred<void>();
    mocks.sessionPermissionRespond.mockReturnValueOnce(first.promise);

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const allowButton = () => {
      const button = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
      if (button === null) throw new Error("allow control did not render");
      return button;
    };
    const label = () => container.querySelector(".permission-card-label")?.textContent ?? "";

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
    await act(async () => allowButton().click());

    await act(async () => {
      root.render(
        <PermissionCard
          sessionId="session-1"
          subscriptionId={42}
          request={request}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    // The abandoned answer finally arrives; it must not stamp the card "allowed".
    await act(async () => {
      first.resolve();
      await first.promise;
    });
    expect(label()).toBe("Waiting on you");
    expect(allowButton().disabled).toBe(false);

    // And the fresh answer still goes out on the new subscription.
    await act(async () => allowButton().click());
    expect(mocks.sessionPermissionRespond).toHaveBeenLastCalledWith(
      "session-1",
      42,
      "tool-a",
      "allow_once",
    );

    await act(async () => root.unmount());
  });

  it("does not let an abandoned rejection reopen the card while the fresh answer is in flight", async () => {
    const first = deferred<void>();
    const second = deferred<void>();
    mocks.sessionPermissionRespond
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);

    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const allowButton = () => {
      const button = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
      if (button === null) throw new Error("allow control did not render");
      return button;
    };
    const label = () => container.querySelector(".permission-card-label")?.textContent ?? "";

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
    await act(async () => allowButton().click());

    await act(async () => {
      root.render(
        <PermissionCard
          sessionId="session-1"
          subscriptionId={42}
          request={request}
          capabilities={["typed_permissions"]}
        />,
      );
    });
    await act(async () => allowButton().click());
    expect(mocks.sessionPermissionRespond).toHaveBeenCalledTimes(2);

    // The abandoned answer fails late; the live answer is still pending, so the
    // card must stay in submitting and not hand the buttons back for a third send.
    await act(async () => {
      first.reject(new Error("abandoned"));
      await Promise.resolve();
    });
    expect(label()).toBe("Sending decision…");
    expect(allowButton().disabled).toBe(true);

    // The live answer fails; now the card may return to an editable state.
    await act(async () => {
      second.reject(new Error("live failed"));
      await Promise.resolve();
    });
    expect(label()).toBe("Waiting on you");
    expect(container.querySelector("[role=alert]")?.textContent).toContain("live failed");

    await act(async () => root.unmount());
  });
});
