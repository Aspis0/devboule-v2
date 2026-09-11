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

import {
  PERMISSION_TARGET_LIMIT,
  PermissionCard,
  permissionSubject,
  shortenPermissionTarget,
} from "./PermissionCard";

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

describe("permissionSubject", () => {
  it("takes the target from the session's wording when the request carries only a tool name", () => {
    // This is the Claude shape: the request says "Read" and the path lives on the
    // tool call, which the daemon correlates by the same toolCallId.
    expect(permissionSubject({ title: "Read" }, "Read src/app/App.tsx")).toEqual({
      action: "Read a file",
      target: "src/app/App.tsx",
    });
  });

  it("translates a tool name that names no target", () => {
    // "Glob" is the case the owner saw live. Nothing on the wire says what it
    // would match, so there is no target to print and none is invented.
    expect(permissionSubject({ title: "Glob" }, "Glob")).toEqual({
      action: "Find files by name",
      target: null,
    });
    expect(permissionSubject({ title: "Glob" })).toEqual({
      action: "Find files by name",
      target: null,
    });
  });

  it("keeps an unknown tool's own wording rather than guessing a translation", () => {
    expect(permissionSubject({ title: "ExitPlanMode" })).toEqual({
      action: "ExitPlanMode",
      target: null,
    });
  });

  it("does not repeat the target under a sentence it could not translate", () => {
    // A title with no known verb keeps the whole sentence, because splitting it
    // would print the path twice and add nothing.
    expect(permissionSubject({ title: "Frobnicate src/a.ts" })).toEqual({
      action: "Frobnicate src/a.ts",
      target: null,
    });
  });

  it("names the action when the request carries no title at all", () => {
    expect(permissionSubject({ title: "" })).toEqual({
      action: "Permission requested",
      target: null,
    });
  });
});

describe("shortenPermissionTarget", () => {
  it("leaves a path that fits alone", () => {
    expect(shortenPermissionTarget("src/app/App.tsx")).toBe("src/app/App.tsx");
  });

  it("keeps both ends of a long path so the filename survives", () => {
    const long =
      "C:/Users/gualt/Desktop/New devboule/devboule-v2-marketplace/src/features/design/DesignSurface.tsx";
    const shortened = shortenPermissionTarget(long);
    expect(shortened.length).toBe(PERMISSION_TARGET_LIMIT);
    expect(shortened).toContain("…");
    expect(shortened.startsWith("C:/Users/gualt")).toBe(true);
    expect(shortened.endsWith("DesignSurface.tsx")).toBe(true);
  });
});

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

  it("says what is being asked about instead of printing the tool's name", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(
        <PermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={{ ...request, title: "Read", description: undefined, command: undefined }}
          toolTitle="Read src/app/App.tsx"
          capabilities={["typed_permissions"]}
        />,
      );
    });

    expect(container.querySelector(".permission-card-action")?.textContent).toBe("Read a file");
    const subject = container.querySelector<HTMLElement>(".permission-card-subject");
    expect(subject?.textContent).toBe("src/app/App.tsx");
    // The full path stays on the element even when the visible text is shortened.
    expect(subject?.title).toBe("src/app/App.tsx");

    await act(async () => root.unmount());
  });

  it("omits the subject line when the wire names no target", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(
        <PermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={{ ...request, title: "Glob", description: undefined, command: undefined }}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    expect(container.querySelector(".permission-card-action")?.textContent).toBe(
      "Find files by name",
    );
    expect(container.querySelector(".permission-card-subject")).toBeNull();

    await act(async () => root.unmount());
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

  it("does not resolve or reopen after the card unmounts mid-answer", async () => {
    const pending = deferred<void>();
    mocks.sessionPermissionRespond.mockReturnValueOnce(pending.promise);
    const onResolved = vi.fn();

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
          onResolved={onResolved}
        />,
      );
    });
    await act(async () => {
      container.querySelector<HTMLButtonElement>(".permission-card-primary-action")?.click();
    });
    await act(async () => root.unmount());
    await act(async () => {
      pending.resolve();
      await pending.promise;
    });
    expect(onResolved).not.toHaveBeenCalled();
  });
});
