import { describe, expect, it } from "vitest";
import { imageMimeType, previewMediaKind } from "./previewMedia";

const IMAGE_SPELLINGS = ["avif", "bmp", "gif", "ico", "jpeg", "jpg", "png", "tiff", "webp"];

describe("imageMimeType", () => {
  it("gives every spelling the panel draws inline a MIME type", () => {
    for (const extension of IMAGE_SPELLINGS) {
      expect(previewMediaKind(`shot.${extension}`)).toBe("image");
      expect(imageMimeType(`shot.${extension}`), extension).not.toBeNull();
    }
  });

  it("matches the extension case-insensitively and by the entry's own name", () => {
    expect(imageMimeType("mockups/SHOT.JPG")).toBe("image/jpeg");
    expect(imageMimeType("dir.d/shot.png")).toBe("image/png");
  });

  it("gives nothing to a file that is not an image spelling", () => {
    expect(imageMimeType("notes.txt")).toBeNull();
    expect(imageMimeType("icon.svg")).toBeNull();
    expect(imageMimeType("dir.d/shot")).toBeNull();
    expect(imageMimeType(".png")).toBeNull();
  });
});
