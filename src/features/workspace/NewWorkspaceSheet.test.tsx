// @vitest-environment happy-dom

// The new-workspace sheet: its groups in the order the choices depend on, what
// each choice sends, the refusal it shows and the answers that close it. The
// agent is not asked for here at all.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { NewWorkspaceSheet, THIS_PC, type NewWorkspaceSheetProps } from "./NewWorkspaceSheet";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const PROJECTS = [
  { id: "p-1", name: "devboule-v2", hostId: THIS_PC },
  { id: "p-2", name: "kalsa-website", hostId: THIS_PC },
  { id: "p-mac", name: "cart-web", hostId: "device-mac" },
];

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  document.body.innerHTML = "";
});

async function render(over: Partial<NewWorkspaceSheetProps> = {}): Promise<{
  onCreate: ReturnType<typeof vi.fn>;
  onClose: ReturnType<typeof vi.fn>;
}> {
  const onCreate = vi.fn(async () => null);
  const onClose = vi.fn();
  await act(async () =>
    root.render(
      <NewWorkspaceSheet
        projects={PROJECTS}
        presetProjectId={null}
        pairedHosts={[]}
        onCreate={onCreate}
        onClose={onClose}
        {...over}
      />,
    ),
  );
  return { onCreate, onClose };
}

function groupLegendsAll(): string[] {
  return [...container.querySelectorAll("legend")].map((legend) => legend.textContent ?? "");
}

function projectRadios(): string[] {
  return [
    ...container.querySelectorAll<HTMLInputElement>('input[name="new-workspace-project"]'),
  ].map((input) => input.value);
}

function groupLegends(): string[] {
  return [...container.querySelectorAll("legend")].map((legend) => legend.textContent ?? "");
}

function radio(name: string, value: string): HTMLInputElement {
  const found = container.querySelector<HTMLInputElement>(
    `input[type="radio"][name="${name}"][value="${value}"]`,
  );
  if (found === null) throw new Error(`no ${name} radio for ${value}`);
  return found;
}

function createButton(): HTMLButtonElement {
  const button = [...container.querySelectorAll<HTMLButtonElement>("button")].find(
    (candidate) => candidate.textContent === "Create workspace",
  );
  if (button === undefined) throw new Error("the Create workspace button did not render");
  return button;
}

async function choose(input: HTMLInputElement): Promise<void> {
  await act(async () => {
    input.click();
  });
}

function setInputValue(input: HTMLInputElement, value: string): void {
  const native = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  native?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

async function submit(): Promise<void> {
  await act(async () => {
    container
      .querySelector("form")
      ?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
  await act(async () => undefined);
}

describe("the new-workspace sheet's groups", () => {
  it("asks for the project, then how it is isolated; no agent and no Where without a paired host", async () => {
    await render();

    expect(groupLegends()).toEqual(["Project", "Isolation"]);
    expect(container.textContent).not.toMatch(/Claude|Codex|Agent|provider/i);
    expect(container.querySelector('input[name="new-workspace-where"]')).toBeNull();
  });

  it("asks where, between this PC and each paired host, once a paired host exists", async () => {
    await render({
      pairedHosts: [{ id: "device-mac", name: "MacBookPro.lan", online: true }],
    });

    expect(groupLegends()).toEqual(["Project", "Where", "Isolation"]);
    expect(radio("new-workspace-where", THIS_PC).checked).toBe(true);
    expect(radio("new-workspace-where", "device-mac").checked).toBe(false);
  });

  it("marks a paired host that does not answer as offline, once", async () => {
    await render({
      pairedHosts: [{ id: "device-mac", name: "MacBookPro.lan", online: false }],
    });

    expect(container.querySelector(".new-workspace-offline")?.textContent).toBe("offline");
  });

  it("starts on the preset project, and on the first one when opened from the top", async () => {
    await render({ presetProjectId: "p-2" });
    expect(radio("new-workspace-project", "p-2").checked).toBe(true);

    await act(async () => root.unmount());
    root = createRoot(container);
    await render({ presetProjectId: null });
    expect(radio("new-workspace-project", "p-1").checked).toBe(true);
  });

  it("asks for a starting branch only for a new worktree", async () => {
    await render();
    expect(container.querySelector("#new-workspace-branch")).toBeNull();

    await choose(radio("new-workspace-isolation", "worktree"));

    expect(container.querySelector("#new-workspace-branch")).not.toBeNull();
  });
});

describe("what the sheet sends", () => {
  it("creates this project's folder workspace with no branch when nothing else is picked", async () => {
    const { onCreate } = await render({ presetProjectId: "p-2" });

    await submit();

    expect(onCreate).toHaveBeenCalledTimes(1);
    expect(onCreate).toHaveBeenCalledWith({
      projectId: "p-2",
      where: THIS_PC,
      isolation: "local",
      branch: null,
    });
  });

  it("sends the typed branch for a worktree, and null when the branch is left blank", async () => {
    const { onCreate } = await render();
    await choose(radio("new-workspace-isolation", "worktree"));
    const branch = container.querySelector<HTMLInputElement>("#new-workspace-branch");
    if (branch === null) throw new Error("the branch field did not render");

    await act(async () => setInputValue(branch, "  feat/sheet  "));
    await submit();
    expect(onCreate).toHaveBeenLastCalledWith({
      projectId: "p-1",
      where: THIS_PC,
      isolation: "worktree",
      branch: "feat/sheet",
    });

    await act(async () => setInputValue(branch, ""));
    await submit();
    expect(onCreate).toHaveBeenLastCalledWith(expect.objectContaining({ branch: null }));
  });

  it("creates once when submitted twice in one tick", async () => {
    const { onCreate } = await render();

    await act(async () => {
      container
        .querySelector("form")
        ?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
      container
        .querySelector("form")
        ?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    });
    await act(async () => undefined);

    expect(onCreate).toHaveBeenCalledTimes(1);
  });

  it("offers only the chosen host's projects, and creates on the host's own project", async () => {
    const { onCreate } = await render({
      pairedHosts: [{ id: "device-mac", name: "MacBookPro.lan", online: true }],
    });
    expect(groupLegendsAll()).toContain("Where");
    expect(projectRadios()).toEqual(["p-1", "p-2"]);

    await choose(radio("new-workspace-where", "device-mac"));

    expect(projectRadios()).toEqual(["p-mac"]);
    expect(radio("new-workspace-project", "p-mac").checked).toBe(true);
    await submit();
    expect(onCreate).toHaveBeenCalledWith({
      projectId: "p-mac",
      where: "device-mac",
      isolation: "local",
      branch: null,
    });
  });

  it("says so when the chosen host lists no project, and creates nothing", async () => {
    const { onCreate } = await render({
      projects: [{ id: "p-1", name: "devboule-v2", hostId: THIS_PC }],
      pairedHosts: [{ id: "device-mac", name: "MacBookPro.lan", online: true }],
    });

    await choose(radio("new-workspace-where", "device-mac"));

    expect(createButton().disabled).toBe(true);
    expect(container.textContent).toContain("That PC lists no project yet.");
    expect(onCreate).not.toHaveBeenCalled();
  });
});

describe("the answers", () => {
  it("shows a refusal as its own sentence and keeps the sheet open", async () => {
    const onCreate = vi.fn(async () => ({
      sentence: "This folder is not a git repository, so it has no worktrees.",
      detail: null,
    }));
    const { onClose } = await render({ onCreate });
    await choose(radio("new-workspace-isolation", "worktree"));

    await submit();

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "not a git repository",
    );
    expect(onClose).not.toHaveBeenCalled();
    expect(container.querySelector('[role="dialog"]')).not.toBeNull();
  });

  it("closes once the workspace exists", async () => {
    const { onClose } = await render();

    await submit();

    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("closes on Cancel and on Escape, with nothing created", async () => {
    const { onCreate, onClose } = await render();

    const cancel = [...container.querySelectorAll<HTMLButtonElement>("button")].find(
      (button) => button.textContent === "Cancel",
    );
    await act(async () => cancel?.click());
    expect(onClose).toHaveBeenCalledTimes(1);

    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(onClose).toHaveBeenCalledTimes(2);
    expect(onCreate).not.toHaveBeenCalled();
  });
});
