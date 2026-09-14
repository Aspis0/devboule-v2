// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PermissionRequest, SessionOriginKind } from "../types/ipc";

const mocks = vi.hoisted(() => ({
  sessionPermissionRespond: vi.fn(),
  reasonFromCause: vi.fn(),
}));

vi.mock("../lib/tauri", () => ({
  sessionPermissionRespond: mocks.sessionPermissionRespond,
  reasonFromCause: mocks.reasonFromCause,
}));

import {
  OUTCOME_BY_OPTION_KIND,
  PERMISSION_TARGET_LIMIT,
  PermissionCard,
  permissionOriginLabel,
  permissionSubject,
  resolutionOutcome,
  shortenAnswererId,
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

describe("shortenAnswererId", () => {
  it("bounds the id by grapheme clusters, not code units (re-audit F12)", () => {
    // Eight emoji are 16 UTF-16 code units — over a unit-based pre-check's
    // limit while being exactly the limit in clusters. The unit version cut
    // half of each pair away and appended an ellipsis; the cluster version
    // returns the id whole, because it fits.
    const eight = "🚀".repeat(8);
    expect(shortenAnswererId(eight)).toBe(eight);
    // Nine emoji (18 units) genuinely overflow: the cut takes WHOLE clusters,
    // so no astral scalar is halved into a lone surrogate rendering U+FFFD.
    expect(shortenAnswererId("🚀".repeat(9))).toBe(`${eight}…`);
  });

  it("leaves a plain ASCII id that fits, as before", () => {
    expect(shortenAnswererId("s.parent")).toBe("s.parent");
    expect(shortenAnswererId("s.creator-session.42")).toBe("s.creato…");
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

  it("says nothing about a local origin, which is not provenance", () => {
    // The daemon stamps `local` on every local request. The card's wording is
    // about the agent, not the machine, so a local session keeps the card it had
    // before peer sessions existed.
    expect(permissionOriginLabel({ kind: "local" })).toBeNull();
  });

  it("calls an absent origin unknown instead of rendering it as a local one", () => {
    // Absent is a third state: only a daemon older than the field sends it, and
    // staying silent would render that request exactly like a local one.
    expect(permissionOriginLabel(undefined)).toBe("Origin: unknown");
    expect(permissionOriginLabel(undefined, new Map())).toBe("Origin: unknown");
  });

  it("calls an origin kind it cannot read unknown instead of rendering it as a local one", () => {
    // The daemon's own word for a journal row whose origin column it could not
    // interpret, and a kind no build has heard of: both are provenance the app
    // does not have. A silent card would say they came from this machine.
    expect(permissionOriginLabel({ kind: "unknown" })).toBe("Origin: unknown");
    expect(permissionOriginLabel({ kind: "kiosk" as SessionOriginKind })).toBe("Origin: unknown");
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

  it("keeps the three origins apart on the card: unknown, local and peer", async () => {
    const renderCard = async (
      origin?: PermissionRequest["origin"],
      deviceNames?: ReadonlyMap<string, string>,
    ) => {
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
            deviceNames={deviceNames}
          />,
        );
      });
      const card = container.querySelector(".permission-card");
      if (card === null) throw new Error("permission card did not render");
      return { card, root };
    };

    // Absent: only an older daemon sends this, and it must not read as local.
    const absent = await renderCard();
    const unknown = absent.card.firstElementChild;
    expect(unknown?.className).toBe("permission-card-origin");
    expect(unknown?.textContent).toBe("Origin: unknown");
    // The request's own text renders below the provenance, in elements of its
    // own: a command that prints a header cannot land in the provenance element.
    expect(absent.card.querySelector(".permission-card-command")?.textContent).toBe(
      "cmd.exe /c echo alpha",
    );

    // Local: the card says nothing, exactly as it did before peer sessions.
    const local = await renderCard({ kind: "local" });
    expect(local.card.querySelector(".permission-card-origin")).toBeNull();
    expect(local.card.firstElementChild?.className).toBe("permission-card-heading");

    // Peer: the device line, in the very element the unknown line uses.
    const peer = await renderCard(
      { kind: "peer", deviceId: "device-1", role: "daemon" },
      new Map([["device-1", "Xiaomi 14"]]),
    );
    expect(peer.card.firstElementChild?.className).toBe("permission-card-origin");
    expect(peer.card.firstElementChild?.textContent).toBe("Device: Xiaomi 14 · Role: daemon");

    await act(async () => absent.root.unmount());
    await act(async () => local.root.unmount());
    await act(async () => peer.root.unmount());
  });

  it("draws an origin kind it cannot read as the unknown line, never as a local card", async () => {
    // The daemon's `unknown` for a journal row it could not interpret, and a
    // kind this build has never heard of: both take the line an absent origin
    // gets, in the same element, because the one thing they may not be is the
    // machine the card is running on.
    for (const kind of ["unknown", "kiosk"] as SessionOriginKind[]) {
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
            origin={{ kind }}
          />,
        );
      });

      const provenance = container.querySelector(".permission-card-origin");
      expect(provenance?.className).toBe("permission-card-origin");
      expect(provenance?.textContent).toBe("Origin: unknown");
      // And it leads the card, where a peer device line would be.
      expect(container.querySelector(".permission-card")?.firstElementChild).toBe(provenance);

      await act(async () => root.unmount());
      container.remove();
    }
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

describe("creator-attributed resolution", () => {
  it("renders the allowed label when the answerer IS the roster's creator", async () => {
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
          resolution={{ outcome: "allowed", answeredBy: "s.creator.1" }}
          creatorId="s.creator.1"
        />,
      );
    });

    const card = container.querySelector(".permission-card");
    if (card === null) throw new Error("permission card did not render");
    // Rendered output, not the event: the label is what the human reads.
    expect(card.querySelector(".permission-card-label")?.textContent).toBe(
      "Allowed by its creator · running",
    );
    const deny = card.querySelector<HTMLButtonElement>(".permission-card-deny-action");
    const allow = card.querySelector<HTMLButtonElement>(".permission-card-primary-action");
    // The human-answer controls are GONE, not disabled: with them removed
    // there is nothing to grey out and nothing to misread as answerable.
    expect(deny).toBeNull();
    expect(allow).toBeNull();
    // And the one control a resolved card keeps is the clear.
    const clear = card.querySelector<HTMLButtonElement>(".permission-card-dismiss-action");
    expect(clear).not.toBeNull();

    await act(async () => root.unmount());
  });

  it("renders the allowed-by-creator wording ONLY for a checked match — a different answerer is named as a session", async () => {
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
          resolution={{ outcome: "allowed", answeredBy: "s.other.99" }}
          creatorId="s.creator.1"
        />,
      );
    });

    const card = container.querySelector(".permission-card");
    if (card === null) throw new Error("permission card did not render");
    expect(card.querySelector(".permission-card-label")?.textContent).toBe(
      "Allowed by session s.other.… · running",
    );
    expect(card.querySelector(".permission-card-label")?.textContent).not.toContain("its creator");
    // The full answerer id stays on the label's title.
    expect(card.querySelector(".permission-card-label")?.getAttribute("title")).toBe("s.other.99");

    await act(async () => root.unmount());
  });

  it("cannot claim a creator the roster does not know: an unknown creator also renders the named session", async () => {
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
          // No creatorId: nobody checked, so nobody claims.
          resolution={{ outcome: "allowed", answeredBy: "s.creator.1" }}
        />,
      );
    });

    const card = container.querySelector(".permission-card");
    if (card === null) throw new Error("permission card did not render");
    expect(card.querySelector(".permission-card-label")?.textContent).toBe(
      "Allowed by session s.creato… · running",
    );

    await act(async () => root.unmount());
  });

  it("never reads 'a person answered' into the daemon's silence: an unnamed answer is its own state", async () => {
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
          resolution={{ outcome: "denied", answeredBy: null }}
          creatorId="s.creator.1"
        />,
      );
    });

    const card = container.querySelector(".permission-card");
    if (card === null) throw new Error("permission card did not render");
    expect(card.querySelector(".permission-card-label")?.textContent).toBe(
      "Denied — the daemon did not say who answered — the turn continues without it",
    );
    expect(card.querySelector(".permission-card-label")?.textContent).not.toContain("creator");
    // The card STAYS for the human to read (and clear), it does not vanish
    // the way the old absent-answeredBy path removed it.
    expect(card.querySelector(".permission-card-dismiss-action")).not.toBeNull();

    await act(async () => root.unmount());
  });

  it("claims no outcome the daemon did not name: allow_always is allowed, an unknown kind is neither", async () => {
    // The outcome table walks the daemon's closed vocabulary; the NOTE's
    // mutation is that flipping allow_always's row must fail these.
    expect(OUTCOME_BY_OPTION_KIND.allow_once).toBe("allowed");
    expect(OUTCOME_BY_OPTION_KIND.allow_always).toBe("allowed");
    expect(OUTCOME_BY_OPTION_KIND.reject_once).toBe("denied");
    expect(OUTCOME_BY_OPTION_KIND.reject_always).toBe("denied");
    expect(resolutionOutcome("allow_always")).toBe("allowed");
    expect(resolutionOutcome("reject_always")).toBe("denied");
    expect(resolutionOutcome("never_heard_of_it")).toBeNull();
    expect(resolutionOutcome(undefined)).toBeNull();
  });

  it("renders an unclaimed outcome as its own state — the card does not invent a decision", async () => {
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
          resolution={{ outcome: null, answeredBy: "s.creator.1" }}
          creatorId="s.creator.1"
        />,
      );
    });

    const card = container.querySelector(".permission-card");
    if (card === null) throw new Error("permission card did not render");
    expect(card.querySelector(".permission-card-label")?.textContent).toBe(
      "Answered by session s.creato… — allowed or denied, the daemon did not say",
    );
    // The dot wears the neutral tone, not the denial's colour.
    expect(card.querySelector(".permission-card-unclaimed")).not.toBeNull();

    await act(async () => root.unmount());
  });

  it("renders an unnamed, unclaimed answer without inventing either fact", async () => {
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
          resolution={{ outcome: null, answeredBy: null }}
        />,
      );
    });

    const card = container.querySelector(".permission-card");
    if (card === null) throw new Error("permission card did not render");
    expect(card.querySelector(".permission-card-label")?.textContent).toBe(
      "Answered — by whom and with what outcome, the daemon did not say",
    );

    await act(async () => root.unmount());
  });

  it("gives a resolved card its one removal path: Clear fires onResolved", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const onResolved = vi.fn();

    await act(async () => {
      root.render(
        <PermissionCard
          sessionId="session-1"
          subscriptionId={41}
          request={request}
          capabilities={["typed_permissions"]}
          resolution={{ outcome: "allowed", answeredBy: "s.creator.1" }}
          creatorId="s.creator.1"
          onResolved={onResolved}
        />,
      );
    });

    const clear = container.querySelector<HTMLButtonElement>(".permission-card-dismiss-action");
    if (clear === null) throw new Error("clear button did not render");
    await act(async () => clear.click());
    expect(onResolved).toHaveBeenCalledWith("session-1", "tool-a");

    await act(async () => root.unmount());
  });

  it("carries the attribution on a denial too — that is the event the roster reader needs", async () => {
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
          resolution={{ outcome: "denied", answeredBy: "s.creator.1" }}
          creatorId="s.creator.1"
        />,
      );
    });

    const card = container.querySelector(".permission-card");
    if (card === null) throw new Error("permission card did not render");
    expect(card.querySelector(".permission-card-label")?.textContent).toBe(
      "Denied by its creator — the turn continues without it",
    );
    // The unattributed human denial must not stand in for it.
    expect(card.querySelector(".permission-card-label")?.textContent).not.toBe(
      "Denied — the turn continues without it",
    );

    await act(async () => root.unmount());
  });

  it("lets the human answer when no outside resolution exists, exactly as before", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    mocks.sessionPermissionRespond.mockResolvedValue(undefined);

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

    const allow = container.querySelector<HTMLButtonElement>(".permission-card-primary-action");
    if (allow === null) throw new Error("allow button did not render");
    expect(allow.disabled).toBe(false);
    await act(async () => allow.click());
    expect(container.querySelector(".permission-card-label")?.textContent).toBe(
      "Allowed once · running",
    );

    await act(async () => root.unmount());
  });
});
