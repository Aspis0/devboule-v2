// Which sessions this window's person started, and whether a remembered pick
// has already been put into one: the answer that decides if a surface may still
// switch a session it is showing.

import { describe, expect, it } from "vitest";
import { forgetCreatedSession, mayApplyPicks, recordCreatedSession } from "./createdSessions";

// The record lives for the window, so every id below is unique to its own case:
// none of them can see another's.

// A case that wants a fresh record for the same id calls resetCreatedSessionsForTests.

describe("a session this window started", () => {
  it("may be asked once, while the roster still shows the generation it started on", () => {
    recordCreatedSession("s-new", 1);

    expect(mayApplyPicks("s-new", 1)).toBe(true);
  });

  it("may not be asked at all when this window never started it", () => {
    recordCreatedSession("s-new", 1);

    expect(mayApplyPicks("s-other", 1)).toBe(false);
  });

  it("may not be asked once a resume moved its generation", () => {
    recordCreatedSession("s-resumed", 1);

    expect(mayApplyPicks("s-resumed", 2)).toBe(false);
  });

  it("may not be asked while the roster row for it has not arrived", () => {
    recordCreatedSession("s-no-row", 1);

    expect(mayApplyPicks("s-no-row", null)).toBe(false);
  });

  it("is asked for once and never again, whatever else happens to the surface", () => {
    recordCreatedSession("s-once", 1);
    expect(mayApplyPicks("s-once", 1)).toBe(true);

    forgetCreatedSession("s-once");

    expect(mayApplyPicks("s-once", 1)).toBe(false);
  });

  it("keeps its record while the roster row is missing, so a late one still counts", () => {
    recordCreatedSession("s-late-row", 1);
    expect(mayApplyPicks("s-late-row", null)).toBe(false);

    expect(mayApplyPicks("s-late-row", 1)).toBe(true);
  });
});
