// @vitest-environment happy-dom

// Why this file exists: the "+" focus rule and the terminal autofocus guard
// ask the same question — is focus still where the strip's flow left it? The
// answer is a pure comparison of elements, so it is proven here rather than
// only through the DOM wiring that consumes it.

import { describe, expect, it } from "vitest";
import { focusIsWhereTheFlowLeftIt } from "./stripFocus";

describe("focusIsWhereTheFlowLeftIt", () => {
  const addButton = document.createElement("button");
  const elsewhere = document.createElement("button");

  it("is true for body: the flow dropped focus and nobody picked it up", () => {
    expect(focusIsWhereTheFlowLeftIt(document.body, addButton)).toBe(true);
  });

  it("is true for a null activeElement", () => {
    expect(focusIsWhereTheFlowLeftIt(null, addButton)).toBe(true);
  });

  it("is true while focus still sits on +", () => {
    expect(focusIsWhereTheFlowLeftIt(addButton, addButton)).toBe(true);
  });

  it("is false once the user has focused something else", () => {
    document.body.appendChild(elsewhere);
    elsewhere.focus();
    expect(focusIsWhereTheFlowLeftIt(elsewhere, addButton)).toBe(false);
    elsewhere.remove();
  });
});
