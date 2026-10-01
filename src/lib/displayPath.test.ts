import { describe, expect, it } from "vitest";
import { displayPath } from "./displayPath";

describe("displayPath", () => {
  it.each([
    [String.raw`\\?\C:\Users\me\project`, String.raw`C:\Users\me\project`],
    [String.raw`\\?\c:\Users\me\project`, String.raw`c:\Users\me\project`],
    [String.raw`\\?\Z:`, "Z:"],
    [String.raw`\\?\UNC\server\share\project`, String.raw`\\server\share\project`],
    [String.raw`\\?\unc\server\share\project`, String.raw`\\server\share\project`],
    [String.raw`\\?\Unc\server\share`, String.raw`\\server\share`],
    [String.raw`\\?\Volume{9f2c}\folder`, String.raw`\\?\Volume{9f2c}\folder`],
    [String.raw`\\?\GLOBALROOT\Device\HarddiskVol`, String.raw`\\?\GLOBALROOT\Device\HarddiskVol`],
    [String.raw`\\?\other\path`, String.raw`\\?\other\path`],
    [String.raw`\\?\1:\folder`, String.raw`\\?\1:\folder`],
    [String.raw`\\?\UNC`, String.raw`\\?\UNC`],
    ["\\\\?\\", "\\\\?\\"],
    [String.raw`\\.\PhysicalDrive0`, String.raw`\\.\PhysicalDrive0`],
    [String.raw`\\.\COM3`, String.raw`\\.\COM3`],
    ["//?/C:/Users/me", "//?/C:/Users/me"],
    ["//?/UNC/server/share", "//?/UNC/server/share"],
    [String.raw`C:\Users\me\project`, String.raw`C:\Users\me\project`],
    [String.raw`\\server\share\project`, String.raw`\\server\share\project`],
    ["/home/me/project", "/home/me/project"],
    ["src/project file.ts", "src/project file.ts"],
    [String.raw`C:\project\literal\\?\tail`, String.raw`C:\project\literal\\?\tail`],
    ["", ""],
    ["   ", "   "],
  ])("displays %s as %s", (path, expected) => {
    expect(displayPath(path)).toBe(expected);
  });
});
