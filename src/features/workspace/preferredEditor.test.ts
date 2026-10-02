// @vitest-environment happy-dom

import { beforeEach, describe, expect, it } from "vitest";
import {
  readPreferredEditorId,
  resolvePreferredEditorId,
  writePreferredEditorId,
} from "./preferredEditor";

const IDS = ["cursor", "vscode", "file-manager"];

describe("the remembered editor choice", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("keeps the saved id while it is still one of the machine's targets", () => {
    expect(resolvePreferredEditorId("vscode", IDS)).toBe("vscode");
  });

  it("falls back to the first available target when the saved id is gone", () => {
    expect(resolvePreferredEditorId("zed", IDS)).toBe("cursor");
  });

  it("falls back to the first available target when nothing is saved", () => {
    expect(resolvePreferredEditorId(null, IDS)).toBe("cursor");
  });

  it("answers null when this machine has no target at all", () => {
    expect(resolvePreferredEditorId("vscode", [])).toBeNull();
    expect(resolvePreferredEditorId(null, [])).toBeNull();
  });

  it("round-trips one id through storage", () => {
    writePreferredEditorId("vscode");
    expect(readPreferredEditorId()).toBe("vscode");
  });

  it("reads corrupt storage as no choice rather than throwing", () => {
    localStorage.setItem("devboule.preferredEditor", "{not json");
    expect(readPreferredEditorId()).toBeNull();
    localStorage.setItem("devboule.preferredEditor", JSON.stringify(41));
    expect(readPreferredEditorId()).toBeNull();
  });
});
