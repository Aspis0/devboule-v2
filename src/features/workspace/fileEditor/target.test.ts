import { describe, expect, it } from "vitest";
import { LOCAL_HOST_ID, workspaceKey, type HostId, type WorkspaceKey } from "../hosts/hostIdentity";
import { isOutsidePath, resolveEditableTarget } from "./useEditableFile";

const local = (workspaceId: string): WorkspaceKey => workspaceKey(LOCAL_HOST_ID, workspaceId)!;
const remote = (workspaceId: string): WorkspaceKey =>
  workspaceKey("device-9" as HostId, workspaceId)!;

describe("editable file routing", () => {
  it("reads and writes a workspace path on the local daemon", () => {
    expect(resolveEditableTarget(local("w.1"), "src/a.ts")).toEqual({
      kind: "workspace",
      workspaceId: "w.1",
      path: "src/a.ts",
    });
  });

  it("relays a paired host's workspace over the held link", () => {
    expect(resolveEditableTarget(remote("w.9"), "src/a.ts")).toEqual({
      kind: "remote",
      deviceId: "device-9",
      workspaceId: "w.9",
      path: "src/a.ts",
    });
  });

  it.each([
    "/home/u/note.md",
    "C:/Users/u/note.md",
    "C:\\Users\\u\\note.md",
    "\\\\server\\share\\note.md",
    "~/.config/pubvia/anthropic.env",
    "~/note.md",
    "  /home/u/note.md  ",
  ])("routes the human path %s app-only", (path) => {
    expect(isOutsidePath(path)).toBe(true);
    expect(resolveEditableTarget(local("w.1"), path)).toEqual({
      kind: "outside",
      path: path.trim(),
    });
    // A remote key never hijacks a human path: the file is on this
    // machine, whichever workspace tab opened it.
    expect(resolveEditableTarget(remote("w.9"), path).kind).toBe("outside");
  });

  it.each(["src/a.ts", "./a.ts", "../a.ts", "a.ts", "C:notdrive/x.ts"])(
    "keeps the workspace spelling %s on the workspace road",
    (path) => {
      expect(isOutsidePath(path)).toBe(false);
    },
  );
});
