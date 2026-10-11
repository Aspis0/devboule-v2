import { describe, expect, it, vi } from "vitest";
import type { WorkspaceEditableFile } from "../../types/ipc";
import { openChatFileLink } from "./chatFileOpen";

function answer(error: string | null): WorkspaceEditableFile {
  return {
    status: error === null ? "ok" : "refused",
    content: null,
    hasBom: null,
    version: null,
    size: null,
    error,
  };
}

describe("openChatFileLink", () => {
  it("answers folder and opens no tab when the editor refuses a folder", async () => {
    const openTab = vi.fn();
    const probe = vi.fn().mockResolvedValue(answer("the requested path is a folder, not a file"));

    await expect(openChatFileLink("mockups/screenshots", probe, openTab)).resolves.toBe("folder");
    expect(probe).toHaveBeenCalledWith("mockups/screenshots");
    expect(openTab).not.toHaveBeenCalled();
  });

  it("opens the file tab as before for a file", async () => {
    const openTab = vi.fn();

    await expect(
      openChatFileLink("src/app.ts", vi.fn().mockResolvedValue(answer(null)), openTab),
    ).resolves.toBe("file");
    expect(openTab).toHaveBeenCalledWith("src/app.ts");
  });

  it("opens the file tab for any other refusal, which the tab then shows", async () => {
    const openTab = vi.fn();

    await openChatFileLink(
      "secret.txt",
      vi.fn().mockResolvedValue(answer("the requested path is outside this workspace")),
      openTab,
    );
    expect(openTab).toHaveBeenCalledWith("secret.txt");
  });

  it("opens the file tab when the editor cannot answer", async () => {
    const openTab = vi.fn();

    await expect(
      openChatFileLink("src/app.ts", vi.fn().mockRejectedValue(new Error("daemon gone")), openTab),
    ).resolves.toBe("file");
    expect(openTab).toHaveBeenCalledWith("src/app.ts");
  });
});
