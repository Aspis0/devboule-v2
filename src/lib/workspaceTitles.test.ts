// @vitest-environment node
import { describe, expect, it } from "vitest";
import { validateWorkspaceTitle, workspaceDisplayTitles } from "./workspaceTitles";

const row = (id: string, title: string) => ({ id, title });

describe("workspaceDisplayTitles", () => {
  it("keeps the first row's title bare and numbers the repeat that follows it", () => {
    const titles = workspaceDisplayTitles([row("w1", "devboule-v2"), row("w2", "devboule-v2")]);
    expect(titles.get("w1")).toBe("devboule-v2");
    expect(titles.get("w2")).toBe("devboule-v2 2");
  });

  it("numbers a third repeat after the second", () => {
    const titles = workspaceDisplayTitles([
      row("w1", "devboule-v2"),
      row("w2", "devboule-v2"),
      row("w3", "devboule-v2"),
    ]);
    expect(titles.get("w3")).toBe("devboule-v2 3");
  });

  it("steps past a number another row already holds", () => {
    const titles = workspaceDisplayTitles([
      row("w1", "devboule-v2"),
      row("w2", "devboule-v2"),
      row("w3", "devboule-v2 2"),
    ]);
    expect(titles.get("w3")).toBe("devboule-v2 2");
    expect(titles.get("w2")).toBe("devboule-v2 3");
  });

  it("leaves rows whose titles already differ alone", () => {
    const titles = workspaceDisplayTitles([row("w1", "docs"), row("w2", "rust")]);
    expect(titles.get("w1")).toBe("docs");
    expect(titles.get("w2")).toBe("rust");
  });
});

describe("validateWorkspaceTitle", () => {
  it("refuses a title that is empty once trimmed", () => {
    expect(validateWorkspaceTitle("   ")).toBe("A workspace title is required; it was empty.");
  });

  it("refuses a title holding invisible formatting", () => {
    expect(validateWorkspaceTitle("dev\u{200b}boule")).toBe(
      "A workspace title must not contain an invisible formatting character.",
    );
  });

  it("refuses a title past the sixty-character limit", () => {
    expect(validateWorkspaceTitle("x".repeat(61))).toBe(
      "A workspace title is 61 characters; the limit is 60.",
    );
  });

  it("accepts a plain title", () => {
    expect(validateWorkspaceTitle("devboule-v2 2")).toBeNull();
  });
});
