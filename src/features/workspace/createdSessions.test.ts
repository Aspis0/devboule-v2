// @vitest-environment happy-dom

// Whose pick reaches a session: the person pressed new-agent in this window, at
// the generation it started on, with nothing sent to it yet. Everything else —
// a roster row, a child of an agent, a session that came back under the same id,
// a turn already running — is left exactly as it is.

import { describe, expect, it } from "vitest";
import { recordCreatedSession, wasCreatedHere } from "./createdSessions";

// The record lives for the app, so every id below is unique to its own case:
// none of them can see another's.

describe("a session this window started", () => {
  it("is the person-started one while it is on the generation it started on", () => {
    recordCreatedSession("s-new", 1);

    expect(wasCreatedHere("s-new", 1)).toBe(true);
  });

  it("is not one this window never started", () => {
    recordCreatedSession("s-new", 1);

    expect(wasCreatedHere("s-other", 1)).toBe(false);
  });

  it("is not the same id after a resume moved its generation", () => {
    recordCreatedSession("s-resumed", 1);

    expect(wasCreatedHere("s-resumed", 2)).toBe(false);
  });

  it("matches on the id alone when either side has no generation to compare", () => {
    recordCreatedSession("s-unknown-create", null);
    recordCreatedSession("s-unknown-row", 1);

    expect(wasCreatedHere("s-unknown-create", 3)).toBe(true);
    expect(wasCreatedHere("s-unknown-row", null)).toBe(true);
  });
});
