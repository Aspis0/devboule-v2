import { describe, expect, it } from "vitest";
import type { PermissionRequest } from "../../types/ipc";
import { pendingPlanId, type RenderedPermissionCard } from "./pendingPlanId";

function planCard(toolCallId: string): RenderedPermissionCard {
  const request: PermissionRequest = {
    type: "permission_request",
    toolCallId,
    title: "Plan",
    options: [
      { optionId: "deny", name: "Reject", kind: "reject_once" },
      { optionId: "implement", name: "Implement", kind: "allow_once" },
    ],
    kind: "plan",
    plan: "## Steps",
  };
  return { request };
}

describe("pendingPlanId", () => {
  it("names the row behind an unanswered plan card", () => {
    expect(pendingPlanId(planCard("plan-1"))).toBe("plan-1");
  });

  it("a resolved card hides nothing", () => {
    expect(pendingPlanId({ ...planCard("plan-1"), resolution: { outcome: "allowed" } })).toBeNull();
  });

  it("no card at all hides nothing", () => {
    expect(pendingPlanId(null)).toBeNull();
  });

  it("a non-plan card never hides a row", () => {
    const toolRequest: PermissionRequest = {
      type: "permission_request",
      toolCallId: "t-1",
      title: "Allow Codex to run this command?",
      options: [
        { optionId: "allow", name: "Allow once", kind: "allow_once" },
        { optionId: "deny", name: "Deny", kind: "reject_once" },
      ],
    };
    expect(pendingPlanId({ request: toolRequest })).toBeNull();
    expect(pendingPlanId({ request: { ...planCard("q-1").request, kind: "question" } })).toBeNull();
  });

  it("an id that is not a non-empty string never hides a row", () => {
    expect(pendingPlanId(planCard(""))).toBeNull();
    expect(
      pendingPlanId({ request: { ...planCard("x").request, toolCallId: 7 as unknown as string } }),
    ).toBeNull();
  });
});
