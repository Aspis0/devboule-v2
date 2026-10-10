import { describe, expect, it } from "vitest";
import { LOCAL_HOST_ID, workspaceKey, type HostId, type WorkspaceKey } from "../hosts/hostIdentity";
import { isAbsolutePath, isOutsidePath, resolveEditableTarget } from "./useEditableFile";

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

  it.each(["~/.config/pubvia/anthropic.env", "~/note.md"])(
    "routes the home path %s app-only",
    (path) => {
      expect(isOutsidePath(path)).toBe(true);
      expect(isAbsolutePath(path)).toBe(false);
      expect(resolveEditableTarget(local("w.1"), path)).toEqual({
        kind: "outside",
        path: path.trim(),
      });
      // `~` always means this machine's home, even under a remote key:
      // the far home is not addressable, and guessing it would open the
      // wrong file.
      expect(resolveEditableTarget(remote("w.9"), path).kind).toBe("outside");
    },
  );

  it.each([
    "/home/u/note.md",
    "C:/Users/u/note.md",
    "C:\\Users\\u\\note.md",
    "\\\\server\\share\\note.md",
    "  /home/u/note.md  ",
  ])("routes the absolute path %s to a workspace road", (path) => {
    expect(isOutsidePath(path)).toBe(false);
    expect(isAbsolutePath(path)).toBe(true);
    // Local: the workspace road, which maps inside spellings itself and
    // refuses outside ones (the hook retries those on the app road).
    expect(resolveEditableTarget(local("w.1"), path)).toEqual({
      kind: "workspace",
      workspaceId: "w.1",
      path,
    });
    // Remote: the held link, confined by the far daemon.
    expect(resolveEditableTarget(remote("w.9"), path)).toEqual({
      kind: "remote",
      deviceId: "device-9",
      workspaceId: "w.9",
      path,
    });
  });

  it.each(["src/a.ts", "./a.ts", "../a.ts", "a.ts", "C:notdrive/x.ts"])(
    "keeps the workspace spelling %s on the workspace road",
    (path) => {
      expect(isOutsidePath(path)).toBe(false);
    },
  );
});
