import { describe, expect, it } from "vitest";
import { isImeComposition } from "./imeComposition";

describe("isImeComposition", () => {
  it("claims a keydown the engine marks as composing", () => {
    expect(isImeComposition({ isComposing: true, keyCode: 13 })).toBe(true);
  });

  it("claims the legacy composition commit that reports only keyCode 229", () => {
    expect(isImeComposition({ isComposing: false, keyCode: 229 })).toBe(true);
  });

  it("leaves a plain Enter outside a composition to the app", () => {
    expect(isImeComposition({ isComposing: false, keyCode: 13 })).toBe(false);
  });

  it("leaves a plain Escape outside a composition to the app", () => {
    expect(isImeComposition({ isComposing: false, keyCode: 27 })).toBe(false);
  });
});
