// The client mirror of the daemon's session-name rule (protocol
// messages.rs:961): the same refusals with the same sentences, so a name the
// client refuses never reaches the daemon and a name the daemon refuses reads
// the same whichever side said it. The daemon trims before storing, so the
// trimmed value is the one judged and the one the caller sends.

import { describe, expect, it } from "vitest";
import {
  SESSION_DISPLAY_NAME_MAX_CHARS,
  unsafeCharacterName,
  validateSessionRename,
} from "./sessionRename";

describe("validateSessionRename", () => {
  it("accepts a plain name", () => {
    expect(validateSessionRename("worker one")).toBeNull();
  });

  it("accepts a name at the exact limit", () => {
    expect(validateSessionRename("x".repeat(SESSION_DISPLAY_NAME_MAX_CHARS))).toBeNull();
  });

  it("judges the trimmed value — the daemon stores the trimmed value", () => {
    expect(validateSessionRename("  worker  ")).toBeNull();
  });

  it("refuses an empty name with the daemon's sentence", () => {
    expect(validateSessionRename("")).toBe("A session display name is required; it was empty.");
  });

  it("refuses a whitespace-only name with the same sentence", () => {
    expect(validateSessionRename("   ")).toBe("A session display name is required; it was empty.");
  });

  it("refuses a name past the limit, naming the length and the limit", () => {
    expect(validateSessionRename("x".repeat(SESSION_DISPLAY_NAME_MAX_CHARS + 1))).toBe(
      `A session display name is ${SESSION_DISPLAY_NAME_MAX_CHARS + 1} characters; the limit is ${SESSION_DISPLAY_NAME_MAX_CHARS}.`,
    );
  });

  it("counts characters, not code units — an astral scalar is one character", () => {
    expect(validateSessionRename("\u{1f3c2}".repeat(SESSION_DISPLAY_NAME_MAX_CHARS))).toBeNull();
    expect(validateSessionRename("\u{1f3c2}".repeat(SESSION_DISPLAY_NAME_MAX_CHARS + 1))).toBe(
      `A session display name is ${SESSION_DISPLAY_NAME_MAX_CHARS + 1} characters; the limit is ${SESSION_DISPLAY_NAME_MAX_CHARS}.`,
    );
  });

  it("judges the value that will be sent — edge U+FEFF is dropped, not refused", () => {
    // JS trim() strips U+FEFF where Rust's str::trim() does not, so the
    // mirror and the daemon judge the same string: the trimmed value is
    // what the dialog sends, and a clean value the daemon accepts. (A
    // middle U+FEFF is caught by unsafeCharacterName on both sides.)
    expect(validateSessionRename("\u{feff}abc")).toBeNull();
    expect(validateSessionRename("abc\u{feff}")).toBeNull();
    expect(validateSessionRename("a\u{feff}b")).toBe(
      "A session display name must not contain an invisible formatting character.",
    );
  });
});

describe("unsafeCharacterName", () => {
  it("finds a control character", () => {
    expect(unsafeCharacterName("wor\u{0001}ker")).toBe("a control character");
  });

  it("finds an invisible formatting character", () => {
    expect(unsafeCharacterName("wor\u{200b}ker")).toBe("an invisible formatting character");
    expect(unsafeCharacterName("\u{feff}")).toBe("an invisible formatting character");
  });

  it("finds a line break the daemon names as one — control is judged first", () => {
    // The daemon checks control before line break (text_safety.rs:34), so
    // \r, \n, \v, \f and NEL report as control; only the two Unicode
    // line separators reach the line-break arm.
    expect(unsafeCharacterName("wor\u{2028}ker")).toBe("a line break character");
    expect(unsafeCharacterName("wor\u{2029}ker")).toBe("a line break character");
    expect(unsafeCharacterName("wor\nker")).toBe("a control character");
  });

  it("passes a plain name", () => {
    expect(unsafeCharacterName("worker one")).toBeNull();
  });
});
