// The slash-menu ranker: which tier a query's match lands in, the tie-break
// inside a tier, the match set it keeps (today's substring set, no more), and
// the empty query's source order.

import { describe, expect, it } from "vitest";
import { rankCommandMatches } from "./commandMatch";

function named(...names: string[]) {
  return names.map((name) => ({ name, description: `${name} description` }));
}

function ranked(names: string[], query: string): string[] {
  return rankCommandMatches(named(...names), query).map((command) => command.name);
}

describe("rankCommandMatches", () => {
  it("orders exact above prefix, word-start and substring", () => {
    expect(ranked(["manual", "woman", "re-man", "man"], "man")).toEqual([
      "man",
      "manual",
      "re-man",
      "woman",
    ]);
  });

  it("orders prefix above word-start", () => {
    expect(ranked(["re-man", "manor"], "man")).toEqual(["manor", "re-man"]);
  });

  it("orders word-start above substring", () => {
    expect(ranked(["woman", "re-man"], "man")).toEqual(["re-man", "woman"]);
  });

  it("takes the best tier across occurrences, not the first hit", () => {
    expect(ranked(["human", "woman-man"], "man")).toEqual(["woman-man", "human"]);
  });

  it("takes a hit after each of -, _, :, . and / as a word start", () => {
    for (const separator of ["-", "_", ":", ".", "/"]) {
      expect(ranked(["woman", `re${separator}man`], "man")).toEqual([`re${separator}man`, "woman"]);
    }
  });

  it("decides a separator's word-start before the offset and name of substring hits", () => {
    for (const separator of ["-", "_", ":", ".", "/"]) {
      expect(ranked([`re${separator}man`, "human", "arc+man"], "man")).toEqual([
        `re${separator}man`,
        "human",
        "arc+man",
      ]);
    }
  });

  it("keeps +, @ and # out of the word-start tier", () => {
    expect(ranked(["woman-man", "arc+man", "dot@man", "hash#man"], "man")).toEqual([
      "woman-man",
      "arc+man",
      "dot@man",
      "hash#man",
    ]);
  });

  it("breaks a tier tie by name, so a name sorts ahead of its own extensions", () => {
    expect(ranked(["goal-archive", "goal", "goal-board"], "go")).toEqual([
      "goal",
      "goal-archive",
      "goal-board",
    ]);
  });

  it("breaks a substring tie by name", () => {
    expect(ranked(["woman", "human"], "man")).toEqual(["human", "woman"]);
  });

  it("ranks the earlier hit first inside a tier, before name order", () => {
    expect(ranked(["aaaa-abc", "b-abc"], "abc")).toEqual(["b-abc", "aaaa-abc"]);
  });

  it("ranks the earlier substring hit first, before name order", () => {
    expect(ranked(["zabc", "abzabc"], "abc")).toEqual(["zabc", "abzabc"]);
  });

  it("keeps the earliest offset among a name's equal-tier hits", () => {
    expect(ranked(["abc-man", "a-man-b"], "man")).toEqual(["a-man-b", "abc-man"]);
    // Two word-start hits in one name: its first one (offset 2) must beat c-b's 2 on name, not lose on 4.
    expect(ranked(["a-b-b", "c-b"], "b")).toEqual(["a-b-b", "c-b"]);
  });

  it("breaks a tier tie by code-unit order, which no runtime locale can reorder", () => {
    expect(ranked(["goal_x", "goal-archive"], "goal")).toEqual(["goal-archive", "goal_x"]);
  });

  it("breaks a tier tie on the lowercased name before the original", () => {
    expect(ranked(["goalX", "goalb"], "goal")).toEqual(["goalb", "goalX"]);
  });

  it("breaks a case-only tie by the original name's code units", () => {
    expect(ranked(["goal", "Goal"], "goal")).toEqual(["Goal", "goal"]);
  });

  it("matches case-insensitively", () => {
    expect(ranked(["Goal"], "GO")).toEqual(["Goal"]);
  });

  it("keeps today's substring match set: interior hits match, near-misses do not", () => {
    expect(ranked(["workflow"], "rkfl")).toEqual(["workflow"]);
    expect(ranked(["goal"], "gol")).toEqual([]);
    expect(ranked(["build", "goal"], "go")).toEqual(["goal"]);
  });

  it("keeps the source order for the empty query", () => {
    expect(ranked(["zeta", "alpha", "mid"], "")).toEqual(["zeta", "alpha", "mid"]);
  });
});
