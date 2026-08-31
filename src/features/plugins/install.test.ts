import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const open = vi.hoisted(() => vi.fn());
const installPlugin = vi.hoisted(() => vi.fn());
const setState = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/plugin-dialog", () => ({ open }));
vi.mock("../../store/appStore", () => ({
  useAppStore: {
    getState: () => ({ installPlugin }),
    setState,
  },
}));
vi.mock("../../lib/tauri", () => ({
  reasonFromCause: (cause: unknown) => (cause instanceof Error ? cause.message : "failed"),
}));

import { chooseAndInstall } from "./install";

beforeEach(() => {
  vi.resetAllMocks();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("chooseAndInstall", () => {
  it("passes the selected folder to the store install action", async () => {
    open.mockResolvedValue("C:/incoming/polis");
    installPlugin.mockResolvedValue(true);

    await expect(chooseAndInstall("polis", "Polis")).resolves.toBe(true);
    expect(installPlugin).toHaveBeenCalledWith("polis", "C:/incoming/polis");
  });

  it("treats cancelling the folder picker as a non-error", async () => {
    open.mockResolvedValue(null);

    await expect(chooseAndInstall("polis", "Polis")).resolves.toBe(false);
    expect(installPlugin).not.toHaveBeenCalled();
  });

  it("clears a stale install error when a new chooser flow begins", async () => {
    open.mockResolvedValue(null);

    await chooseAndInstall("polis", "Polis");

    expect(setState).toHaveBeenCalledWith({ installError: null });
  });
});
