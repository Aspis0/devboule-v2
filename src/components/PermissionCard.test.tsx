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
  permissionOriginLabel,
  permissionSubject,
  shortenDeviceId,
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

describe("permissionOriginLabel", () => {
  const peerDeviceId = "9f6b0f2e-6f1c-4a1e-9c62-1e2f7d59a9c3";

  it("names the device through the workspace's device map", () => {
    expect(
      permissionOriginLabel(
        { kind: "peer", deviceId: "device-phone", role: "client" },
        new Map([["device-phone", "Xiaomi 14"]]),
      ),
    ).toBe("Device: Xiaomi 14 · Role: client");
  });

  it("shows the head of an unknown device id, never the whole UUID", () => {
    const label = permissionOriginLabel(
      { kind: "peer", deviceId: peerDeviceId, role: "daemon" },
      new Map(),
    );
    expect(label).toBe("Device: 9f6b0f2e… · Role: daemon");
    expect(label).not.toContain(peerDeviceId);
    // An id short enough to read is left alone.
    expect(shortenDeviceId("device-1")).toBe("device-1");
  });

  it("has nothing to say about a local or absent origin", () => {
    // The daemon stamps a local origin on every local request, so a present-
    // but-local origin must render exactly like no origin at all.
    expect(permissionOriginLabel({ kind: "local" })).toBeNull();
    expect(permissionOriginLabel(undefined)).toBeNull();
  });

  it("stands in for a field a peer origin did not carry", () => {
    // A peer origin always carries both fields; this is the guard, not a case
    // in normal use, and the provenance is not allowed to vanish for it.
    expect(permissionOriginLabel({ kind: "peer" })).toBe("Device: unknown · Role: unknown");
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

  it("renders a peer origin's provenance as the card's first line", async () => {
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
          origin={{ kind: "peer", deviceId: "device-1", role: "daemon" }}
          deviceNames={new Map([["device-1", "Xiaomi 14"]])}
        />,
      );
    });

    const card = container.querySelector(".permission-card");
    if (card === null) throw new Error("permission card did not render");
    const provenance = card.firstElementChild;
    expect(provenance?.className).toBe("permission-card-origin");
    expect(provenance?.textContent).toBe("Device: Xiaomi 14 · Role: daemon");
    // The request's own text renders below the provenance, in its own elements:
    // a command that prints a header of its own cannot land in this element.
    expect(card.querySelector(".permission-card-command")?.textContent).toBe(
      "cmd.exe /c echo alpha",
    );

    await act(async () => root.unmount());
  });

  it("reads the origin the daemon put on the request when the host passes none", async () => {
    const unknownDeviceId = "9f6b0f2e-6f1c-4a1e-9c62-1e2f7d59a9c3";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(
        <PermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={{
            ...request,
            origin: { kind: "peer", deviceId: unknownDeviceId, role: "client" },
          }}
          capabilities={["typed_permissions"]}
        />,
      );
    });

    const provenance = container.querySelector(".permission-card-origin")?.textContent ?? "";
    expect(provenance).toBe("Device: 9f6b0f2e… · Role: client");
    // No name resolved, and the raw UUID is not what a person is shown.
    expect(provenance).not.toContain(unknownDeviceId);

    await act(async () => root.unmount());
  });

  it("leaves a card without a peer origin unchanged", async () => {
    const renderCard = async (origin?: PermissionRequest["origin"]) => {
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
            origin={origin}
          />,
        );
      });
      return { container, root };
    };

    const absent = await renderCard();
    const cardWithout = absent.container.querySelector(".permission-card");
    expect(cardWithout?.querySelector(".permission-card-origin")).toBeNull();
    expect(cardWithout?.firstElementChild?.className).toBe("permission-card-heading");
    // The daemon stamps `local` on every local request, so that origin must
    // render exactly like no origin at all.
    const local = await renderCard({ kind: "local" });
    const cardLocal = local.container.querySelector(".permission-card");
    expect(cardLocal?.querySelector(".permission-card-origin")).toBeNull();
    expect(cardLocal?.firstElementChild?.className).toBe("permission-card-heading");

    await act(async () => absent.root.unmount());
    await act(async () => local.root.unmount());
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
