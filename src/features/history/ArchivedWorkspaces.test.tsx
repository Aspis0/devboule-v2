// @vitest-environment happy-dom

// The History page's archived section: what it lists, the two ways back, and
// what a refusal says. The daemon calls are doubles at the module boundary.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ArchivedWorkspaces } from "./ArchivedWorkspaces";
import { archivedList, projectAdd, workspaceRestore } from "../../lib/tauri";
import type { ArchivedList, Workspace } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  archivedList: vi.fn(),
  projectAdd: vi.fn(),
  workspaceRestore: vi.fn(),
}));

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const ARCHIVED_WORKSPACE = {
  id: "w-old",
  projectId: "p-kalsa",
  title: "feat-cart",
  isolation: "worktree",
  path: "C:\\work\\kalsa\\wt",
} as Workspace;

const REMOVED_PROJECT = { id: "p-old", name: "cart-web", path: "C:\\work\\cart-web" };

const LIST: ArchivedList = {
  projects: [REMOVED_PROJECT],
  workspaces: [ARCHIVED_WORKSPACE],
} as ArchivedList;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  vi.mocked(archivedList).mockResolvedValue(LIST);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
});

async function render(onRestored = vi.fn()): Promise<void> {
  await act(async () => root.render(<ArchivedWorkspaces onRestored={onRestored} />));
}

async function open(): Promise<void> {
  const toggle = container.querySelector<HTMLButtonElement>(".history-archived-toggle");
  if (toggle === null) throw new Error("the archived toggle did not render");
  await act(async () => toggle.click());
  await act(async () => undefined);
}

function button(label: string): HTMLButtonElement {
  const found = container.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
  if (found === null) throw new Error(`no button ${label}`);
  return found;
}

describe("the History page's archived section", () => {
  it("reads nothing until it is opened", async () => {
    await render();

    expect(archivedList).not.toHaveBeenCalled();
    expect(container.querySelector(".history-archived-list")).toBeNull();
  });

  it("lists the archived workspace and the removed project, each with its own Restore", async () => {
    await render();
    await open();

    expect(archivedList).toHaveBeenCalledTimes(1);
    expect(container.querySelector(".history-archived-list")?.textContent).toContain("feat-cart");
    expect(container.querySelector(".history-archived-list")?.textContent).toContain("cart-web");
    expect(button("Restore feat-cart")).not.toBeNull();
    expect(button("Restore project cart-web")).not.toBeNull();
  });

  it("puts an archived workspace back on the list and tells the sidebar to re-read it", async () => {
    vi.mocked(workspaceRestore).mockResolvedValue(ARCHIVED_WORKSPACE);
    vi.mocked(archivedList)
      .mockResolvedValueOnce(LIST)
      .mockResolvedValueOnce({
        projects: [REMOVED_PROJECT],
        workspaces: [],
      } as ArchivedList);
    const onRestored = vi.fn();
    await render(onRestored);
    await open();

    await act(async () => button("Restore feat-cart").click());
    await act(async () => undefined);

    expect(workspaceRestore).toHaveBeenCalledWith("w-old");
    expect(onRestored).toHaveBeenCalledTimes(1);
    expect(container.querySelector('button[aria-label="Restore feat-cart"]')).toBeNull();
  });

  it("adds a removed project's folder again to list it, and tells the sidebar", async () => {
    vi.mocked(projectAdd).mockResolvedValue({
      id: "p-old",
      name: "cart-web",
      path: "C:\\work\\cart-web",
    } as never);
    const onRestored = vi.fn();
    await render(onRestored);
    await open();

    await act(async () => button("Restore project cart-web").click());
    await act(async () => undefined);

    expect(projectAdd).toHaveBeenCalledWith("C:\\work\\cart-web");
    expect(onRestored).toHaveBeenCalledTimes(1);
  });

  it("shows a refusal as its own sentence and restores nothing", async () => {
    vi.mocked(workspaceRestore).mockRejectedValue({
      code: "invalid_request",
      message: "Its project was removed from the list: add the project's folder again first.",
    });
    const onRestored = vi.fn();
    await render(onRestored);
    await open();

    await act(async () => button("Restore feat-cart").click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "add the project's folder again first",
    );
    expect(onRestored).not.toHaveBeenCalled();
  });

  it("says so when nothing is archived", async () => {
    vi.mocked(archivedList).mockResolvedValue({ projects: [], workspaces: [] } as ArchivedList);
    await render();
    await open();

    expect(container.textContent).toContain("Nothing is archived.");
  });
});
