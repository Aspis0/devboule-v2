import { describe, expect, it } from "vitest";
import { railTail } from "./railAgentTail";

const NOW = 1_000_000;
const MINUTE = 60_000;

const agent = (over: Partial<Parameters<typeof railTail>[0]> = {}) => ({
  state: "idle" as const,
  turnStartedAtMs: null,
  turnEndedAtMs: null,
  ...over,
});

describe("the rail's tail", () => {
  it("shows the running turn's clock, counted from the roster's start", () => {
    expect(railTail(agent({ state: "running", turnStartedAtMs: NOW - 161_000 }), NOW)).toEqual({
      text: "02:41",
      tone: "muted",
    });
  });

  it("shows no clock for a running agent whose turn start the roster does not give", () => {
    expect(railTail(agent({ state: "running" }), NOW)).toBeNull();
  });

  it("shows how long ago a finished agent's turn ended, with the zero unit dropped", () => {
    expect(railTail(agent({ state: "finished", turnEndedAtMs: NOW - 7 * MINUTE }), NOW)).toEqual({
      text: "7m",
      tone: "muted",
    });
    expect(
      railTail(agent({ state: "finished", turnEndedAtMs: NOW - 7 * MINUTE - 12_000 }), NOW),
    ).toEqual({ text: "7m 12s", tone: "muted" });
    expect(railTail(agent({ state: "finished", turnEndedAtMs: NOW - 45_000 }), NOW)).toEqual({
      text: "45s",
      tone: "muted",
    });
  });

  it("shows no age for a finished agent whose end the roster does not give", () => {
    expect(railTail(agent({ state: "finished" }), NOW)).toBeNull();
  });

  it("names a waiting agent's need in the attention tone, and a failed or stopped one's state", () => {
    expect(railTail(agent({ state: "waiting" }), NOW)).toEqual({
      text: "needs you",
      tone: "attention",
    });
    expect(railTail(agent({ state: "failed" }), NOW)).toEqual({ text: "failed", tone: "danger" });
    expect(railTail(agent({ state: "stopped" }), NOW)).toEqual({ text: "stopped", tone: "muted" });
  });

  it("shows no tail for an idle agent", () => {
    expect(railTail(agent({ state: "idle" }), NOW)).toBeNull();
  });
});
