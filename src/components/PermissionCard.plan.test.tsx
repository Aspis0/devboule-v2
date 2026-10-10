// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PermissionCard } from "./PermissionCard";
import type { PermissionRequest } from "../types/ipc";

const respond = vi.hoisted(() => vi.fn());
vi.mock("../lib/tauri", () => ({ sessionPermissionRespond: respond }));
const styles = vi.hoisted(() => ({ imported: false }));
vi.mock("./PermissionCard.css", () => {
  styles.imported = true;
  return {};
});

const planRequest: PermissionRequest = {
  type: "permission_request",
  toolCallId: "plan-1",
  title: "Plan",
  kind: "plan",
  plan: "## Steps\n\n```text\nlong step\n```\n\n- Add the route\n- **Verify** it",
  options: [
    { optionId: "deny", name: "Reject", kind: "reject_once" },
    { optionId: "implement", name: "Implement", kind: "allow_once" },
  ],
};

describe("plan permission card", () => {
  afterEach(() => {
    document.body.replaceChildren();
    respond.mockReset();
  });

  it("renders the plan on Design with its component styles and action", async () => {
    const container = document.createElement("div");
    container.className = "design-surface";
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        <PermissionCard
          sessionId="session-1"
          subscriptionId={1}
          request={planRequest}
          capabilities={["typed_permissions"]}
        />,
      );
    });
    expect(container.querySelector(".permission-card-action")?.textContent).toBe("Plan");
    expect(
      container.querySelector(".permission-card-plan .plan-markdown-heading-2")?.textContent,
    ).toBe("Steps");
    const heading = container.querySelector(
      '.permission-card-plan [role="heading"][aria-level="2"]',
    );
    expect(heading?.textContent).toBe("Steps");
    expect(container.querySelector(".permission-card-plan h2")).toBeNull();
    expect(container.querySelectorAll(".permission-card-plan li")).toHaveLength(2);
    expect(container.querySelector(".permission-card-plan strong")?.textContent).toBe("Verify");
    expect(styles.imported).toBe(true);
    const implement = [...container.querySelectorAll("button")].find(
      (button) => button.textContent === "Implement",
    );
    expect(implement).toBeDefined();
    expect(
      [...container.querySelectorAll("button")].some((button) => button.textContent === "Reject"),
    ).toBe(true);
    respond.mockResolvedValue(undefined);
    await act(async () => implement?.click());
    expect(respond).toHaveBeenCalledWith(
      "session-1",
      1,
      "plan-1",
      "allow_once",
      "implement",
      undefined,
    );
    await act(async () => root.unmount());
  });

  it("says Approved or Rejected once the plan is answered, never who answered or how", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        <PermissionCard
          sessionId="session-1"
          subscriptionId={1}
          request={planRequest}
          capabilities={["typed_permissions"]}
          resolution={{ outcome: "allowed", answeredBy: null, selectedOptionName: null }}
        />,
      );
    });
    expect(container.querySelector(".permission-card-label")?.textContent).toBe("Approved");
    expect(container.textContent).not.toContain("Answered");
    expect(container.textContent).not.toContain("daemon did not say");
    await act(async () => root.unmount());
  });

  it("says Rejected for a plan the daemon answered with a denial", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        <PermissionCard
          sessionId="session-1"
          subscriptionId={1}
          request={planRequest}
          capabilities={["typed_permissions"]}
          resolution={{ outcome: "denied", answeredBy: null, selectedOptionName: null }}
        />,
      );
    });
    expect(container.querySelector(".permission-card-label")?.textContent).toBe("Rejected");
    await act(async () => root.unmount());
  });

  it("claims no word for a plan answered with an outcome the daemon did not name", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        <PermissionCard
          sessionId="session-1"
          subscriptionId={1}
          request={planRequest}
          capabilities={["typed_permissions"]}
          resolution={{ outcome: null, answeredBy: null, selectedOptionName: null }}
        />,
      );
    });
    expect(container.querySelector(".permission-card-label")?.textContent).toBe("");
    expect(container.querySelector(".permission-card-dismiss-action")?.textContent).toBe("Clear");
    await act(async () => root.unmount());
  });

  it("names no chosen option under a plan", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    respond.mockResolvedValue(undefined);
    await act(async () => {
      root.render(
        <PermissionCard
          sessionId="session-1"
          subscriptionId={1}
          request={planRequest}
          capabilities={["typed_permissions"]}
        />,
      );
    });
    const implement = [...container.querySelectorAll("button")].find(
      (button) => button.textContent === "Implement",
    );
    await act(async () => implement?.click());
    expect(container.querySelector(".permission-card-label")?.textContent).toBe("Approved");
    expect(container.querySelector(".permission-card-choice")).toBeNull();
    expect(container.textContent).not.toContain("Chosen");
    await act(async () => root.unmount());
  });
});
