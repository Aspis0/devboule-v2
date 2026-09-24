import { describe, expect, it } from "vitest";
import { avatarStyle, avatarTone, type AvatarTone } from "./avatars";

describe("avatar tones", () => {
  it("pick one of the mockup's five tones, deterministically from the id", () => {
    const tones = new Set<AvatarTone>();
    for (let index = 0; index < 200; index += 1) {
      const id = `workspace-${index}`;
      expect(avatarTone(id)).toBe(avatarTone(id));
      tones.add(avatarTone(id));
    }
    // The whole palette is in use: the ids spread across all five tones.
    expect(tones.size).toBe(5);
  });

  it("always returns one of the five tones for arbitrary ids", () => {
    for (const id of ["", "a", "(devboule)", "workspace:8F3A-2", "🔌"]) {
      expect(["live", "recovered", "attention", "unattended", "idle"]).toContain(avatarTone(id));
    }
  });

  it("mix the tone over transparent at the mockup's percentages", () => {
    const style = avatarStyle(avatarTone("workspace-1") === "live" ? "workspace-1" : "x");
    expect(style.background).toMatch(
      /color-mix\(in srgb, var\(--tone-[a-z]+\) (20|22|25)%, transparent\)/,
    );
    expect(style.color).toMatch(/var\(--tone-[a-z]+\)/);
  });

  it("keeps the tone identical for the same id everywhere it renders", () => {
    expect(avatarStyle("project-9")).toEqual(avatarStyle("project-9"));
    expect(avatarTone("project-9")).toBe(avatarTone("project-9"));
  });
});
