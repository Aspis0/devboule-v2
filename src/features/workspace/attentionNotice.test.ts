import { describe, expect, it } from "vitest";
import type { Attention } from "../../types/ipc";
import {
  PREVIEW_LIMIT,
  attentionRaised,
  previewFrom,
  stripMarkdown,
  toastContent,
  toastGate,
} from "./attentionNotice";

function attention(reason: Attention["reason"], atMs: number): Attention {
  return { reason, atMs };
}

describe("attentionRaised", () => {
  it("fires exactly once per raise", () => {
    const raise = attention("finished", 1000);
    expect(attentionRaised(undefined, raise)).toBe(true);
    // Harmless roster re-publications of the same raise: silence.
    expect(attentionRaised(raise, raise)).toBe(false);
    expect(attentionRaised(attention("finished", 1000), raise)).toBe(false);
  });

  it("does not fire when attention clears or stays cleared", () => {
    expect(attentionRaised(attention("error", 1000), undefined)).toBe(false);
    expect(attentionRaised(undefined, undefined)).toBe(false);
  });

  it("fires again only for a newer raise, never an older push", () => {
    const first = attention("permission", 1000);
    const second = attention("finished", 2000);
    expect(attentionRaised(first, second)).toBe(true);
    // A late push carrying the older event is not a new event.
    expect(attentionRaised(second, first)).toBe(false);
  });
});

describe("toastGate", () => {
  it("stays silent while the window is focused and visible", () => {
    expect(toastGate(true, true)).toBe(false);
  });

  it("fires when the window is hidden, minimized, or unfocused", () => {
    expect(toastGate(false, false)).toBe(true); // in the tray
    expect(toastGate(false, true)).toBe(true); // minimized
    expect(toastGate(true, false)).toBe(true); // visible but behind
  });
});

describe("stripMarkdown", () => {
  it("keeps plain text untouched", () => {
    expect(stripMarkdown("Build passed in 3m 20s")).toBe("Build passed in 3m 20s");
  });

  it("reduces the constructs to their words", () => {
    expect(stripMarkdown("## Status")).toBe("Status");
    expect(stripMarkdown("see [the docs](https://example.com) now")).toBe("see the docs now");
    expect(stripMarkdown("**done** and *fast* and `fixed`")).toBe("done and fast and fixed");
    expect(stripMarkdown("- one\n- two")).toBe("one\ntwo");
  });

  it("collapses whitespace runs", () => {
    expect(stripMarkdown("a   b\n\n  c")).toBe("a b\nc");
  });
});

describe("previewFrom", () => {
  it("caps the preview around 220 characters on a boundary", () => {
    const long = "word ".repeat(200).trim();
    const preview = previewFrom(long);
    expect(preview.length).toBeLessThanOrEqual(PREVIEW_LIMIT + 1);
    expect(preview.endsWith("…")).toBe(true);
    expect(long.startsWith(preview.slice(0, -1))).toBe(true);
  });

  it("leaves short text alone", () => {
    expect(previewFrom("All checks passed")).toBe("All checks passed");
  });
});

describe("toastContent", () => {
  it("names the session and the reason in the title", () => {
    expect(toastContent("fix login", "permission", undefined).title).toContain("fix login");
    expect(toastContent("fix login", "permission", undefined).title).toContain("needs approval");
  });

  it("carries the held permission request in the body", () => {
    const content = toastContent("fix login", "permission", {
      permissionText: "Run npm install",
    });
    expect(content.body).toBe("Run npm install");
  });

  it("carries the held assistant preview for finished", () => {
    const content = toastContent("fix login", "finished", {
      lastAssistantText: "Deploy finished successfully",
    });
    expect(content.body).toContain("Deploy finished successfully");
  });

  it("falls back to the bare reason when nothing is held", () => {
    expect(toastContent("fix login", "permission", undefined).body).toBe("needs approval");
    expect(toastContent("fix login", "finished", {}).body).toBe("finished");
    expect(toastContent("fix login", "error", undefined).body).toBe("error");
  });

  it("never invents content it does not hold", () => {
    const content = toastContent("fix login", "finished", undefined);
    expect(content.body).toBe("finished");
    expect(content.body.length).toBeLessThan(20);
  });
});
