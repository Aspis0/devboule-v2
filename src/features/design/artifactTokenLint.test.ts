import { describe, expect, it } from "vitest";
import { findUndefinedCustomProperties } from "./artifactTokenLint";

describe("artifact custom-property lint", () => {
  it("reports a referenced token with no definition", () => {
    expect(findUndefinedCustomProperties("<style>.card { color: var(--ink); }</style>")).toEqual([
      "--ink",
    ]);
  });

  it("does not report a referenced token that is defined", () => {
    expect(
      findUndefinedCustomProperties("<style>.card { --ink: #222; color: var(--ink); }</style>"),
    ).toEqual([]);
  });

  it("does not report a missing token when the reference has a fallback", () => {
    expect(findUndefinedCustomProperties("color: var(--ink, 4px)")).toEqual([]);
  });

  it("reports only the unresolved inner token in a fallback chain", () => {
    expect(findUndefinedCustomProperties("color: var(--ink, var(--fallback))")).toEqual([
      "--fallback",
    ]);
  });

  it("collects definitions outside :root", () => {
    expect(
      findUndefinedCustomProperties(
        "<style>@media (min-width: 1px) { .card { --space: 8px; } }</style> color: var(--space)",
      ),
    ).toEqual([]);
  });

  it("reports each missing token once", () => {
    expect(
      findUndefinedCustomProperties(
        "<style>.a { color: var(--ink); }.b { border-color: var(--ink); }</style>",
      ),
    ).toEqual(["--ink"]);
  });

  it("reports references in inline styles and content declarations", () => {
    expect(
      findUndefinedCustomProperties(
        '<div style="color: var(--inline-token)"></div><style>.badge::before { content: var(--content-token); }</style>',
      ),
    ).toEqual(["--inline-token", "--content-token"]);
  });

  it("ignores var references inside CSS comments", () => {
    expect(
      findUndefinedCustomProperties("<style>/* var(--commented) */ .card { color: red; }</style>"),
    ).toEqual([]);
  });

  it("ignores var references inside HTML comments", () => {
    expect(findUndefinedCustomProperties("<!-- var(--commented) --><main>Card</main>")).toEqual([]);
  });

  it("keeps a reference alive when a CSS comment contains a close parenthesis", () => {
    expect(findUndefinedCustomProperties("color: var(--ink /* ) */)")).toEqual(["--ink"]);
  });

  it("does not let comments change a definition and its use", () => {
    expect(
      findUndefinedCustomProperties(
        "<style>.card { --ink: #222; /* var(--missing) */ color: var(--ink); }</style>",
      ),
    ).toEqual([]);
  });

  it("finds a reference after a long comment", () => {
    const longComment = `/* ${"noise ".repeat(200)} */`;
    expect(
      findUndefinedCustomProperties(`${longComment}<style>.card { color: var(--after); }</style>`),
    ).toEqual(["--after"]);
  });

  it("returns nothing when the artifact has no var reference", () => {
    expect(findUndefinedCustomProperties('<main style="color: red">Hello</main>')).toEqual([]);
  });

  it("does not throw on empty or malformed input", () => {
    expect(() => findUndefinedCustomProperties("")).not.toThrow();
    expect(() =>
      findUndefinedCustomProperties("<style>.card { color: var(--broken;"),
    ).not.toThrow();
  });
});
