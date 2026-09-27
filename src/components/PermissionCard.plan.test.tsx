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
});
