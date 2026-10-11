// A Devboule tool call reads as one line: its verb, the agent it touched and what
// that came to. The raw result stays behind the line's click, never on the line.

import { describe, expect, it } from "vitest";
import type { ToolChatItem } from "../../lib/toolCallGroups";
import { devbouleChildSession, devbouleToolLine } from "./devbouleToolLine";

function call(title: string, output = "", status = "completed"): ToolChatItem {
  return {
    id: "t1",
    role: "tool",
    title,
    output,
    toolCallId: "t1",
    status,
  } as ToolChatItem;
}

describe("devbouleToolLine", () => {
  it("reads a message to an agent as one line that names the agent", () => {
    expect(
      devbouleToolLine(call("devboule_send_message", '{"displayName":"Finish Kalsa website"}')),
    ).toBe("Sent a message to Finish Kalsa website");
  });

  it("reads an activity check as the agent's name and its state", () => {
    const output = JSON.stringify({
      sessionId: "s.f4.child",
      displayName: "Finish Kalsa website",
      activity: "working",
      hookSeq: 40,
    });
    expect(devbouleToolLine(call("mcp__devboule__devboule_agent_activity", output))).toBe(
      "Checked Finish Kalsa website: working",
    );
  });

  it("reads a stop as the agent and how long it ran, when the result says so", () => {
    // The shape the daemon's stop result carries.
    const output = JSON.stringify({
      state: "stopped",
      sessionId: "s.kalsa.finish",
      displayName: "Finish Kalsa website",
      runMs: 217_000,
    });
    expect(devbouleToolLine(call("devboule_stop_agent", output))).toBe(
      "Stopped Finish Kalsa website · 3m 37s",
    );
  });

  it("reads a stop without a duration as the agent alone", () => {
    expect(
      devbouleToolLine(call("devboule_stop_agent", '{"displayName":"Finish Kalsa website"}')),
    ).toBe("Stopped Finish Kalsa website");
  });

  it("reads a cancel plainly and a create as the agent's start, with its name", () => {
    expect(devbouleToolLine(call("devboule_cancel_agent", '{"displayName":"Kid"}'))).toBe(
      "Canceled Kid",
    );
    expect(devbouleToolLine(call("devboule_create_agent", '{"displayName":"Kid"}'))).toBe(
      "Started agent · Kid",
    );
    expect(
      devbouleToolLine(call("devboule_create_agent", '{"displayName":"Kid"}'), "running"),
    ).toBe("Starting agent · Kid");
  });

  it("reads a failed create as a sentence that names no agent, since the call has none", () => {
    expect(devbouleToolLine(call("devboule_create_agent", ""), "failed")).toBe(
      "Couldn't start the agent",
    );
  });

  it("names the agent from the call's own words when the result carries no name", () => {
    expect(devbouleToolLine(call("devboule_stop_agent Finish Kalsa website", ""))).toBe(
      "Stopped Finish Kalsa website",
    );
  });

  it("falls back to 'an agent' when neither the result nor the title names one", () => {
    expect(devbouleToolLine(call("devboule_send_message", "not json"))).toBe(
      "Sent a message to an agent",
    );
  });

  it("speaks in the present while the call runs", () => {
    expect(devbouleToolLine(call("devboule_send_message", "", "running"), "running")).toBe(
      "Sending a message to an agent",
    );
    expect(devbouleToolLine(call("devboule_agent_activity", "", "running"), "running")).toBe(
      "Checking an agent",
    );
  });

  it("returns nothing for a tool that is not one of Devboule's agent tools", () => {
    expect(devbouleToolLine(call("cargo test", ""))).toBeUndefined();
    expect(devbouleToolLine(call("devboule_list_profiles", "{}"))).toBeUndefined();
    expect(devbouleToolLine(call("mcp__devboule__browser_click", ""))).toBeUndefined();
  });
});

describe("a call that did not finish", () => {
  it("reads a cancelled start, send or stop as cancelled, never as done", () => {
    expect(
      devbouleToolLine(call("devboule_create_agent", '{"displayName":"Kid"}'), "cancelled"),
    ).toBe("Cancelled · start of Kid");
    expect(
      devbouleToolLine(call("devboule_send_message", '{"displayName":"Kid"}'), "cancelled"),
    ).toBe("Cancelled · message to Kid");
    expect(
      devbouleToolLine(call("devboule_stop_agent", '{"displayName":"Kid"}'), "cancelled"),
    ).toBe("Cancelled · stop of Kid");
  });

  it("names a cancelled start with no agent by what it was for", () => {
    expect(devbouleToolLine(call("devboule_create_agent", ""), "cancelled")).toBe(
      "Cancelled · start of an agent",
    );
  });
});

describe("the child a call's result names", () => {
  it("reads the session id a result carries, and nothing else", () => {
    expect(devbouleChildSession('{"displayName":"Kid","sessionId":"s.kid.1"}')).toBe("s.kid.1");
    expect(devbouleChildSession('{"displayName":"Kid"}')).toBeUndefined();
    expect(devbouleChildSession("Mcp error -32602")).toBeUndefined();
  });
});
