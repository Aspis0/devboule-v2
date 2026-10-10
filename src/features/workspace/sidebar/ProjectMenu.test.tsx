// @vitest-environment happy-dom

// A project's kebab: one entry, Remove project. It asks first, takes the project
// off the list only, and shows a refusal as its own sentence.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ProjectMenu } from "./ProjectMenu";
import type { ErrorSentence } from "../../../lib/errorSentence";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  document.body.innerHTML = "";
});

function kebab(): HTMLButtonElement {
  const button = container.querySelector<HTMLButtonElement>("button.sidebar-kebab");
  if (button === null) throw new Error("the project's kebab did not render");
  return button;
}

function menuItems(): string[] {
  return [...container.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].map(
    (item) => item.textContent ?? "",
  );
}

function dialog(): HTMLElement {
  const found = document.querySelector<HTMLElement>("[role='dialog'], [role='alertdialog']");
  if (found === null) throw new Error("the removal ask did not open");
  return found;
}

function dialogButton(label: string): HTMLButtonElement {
  const button = [...dialog().querySelectorAll<HTMLButtonElement>("button")].find(
    (candidate) => candidate.textContent === label,
  );
  if (button === undefined) throw new Error(`the ask's ${label} button did not render`);
  return button;
}

async function settle(): Promise<void> {
  await act(async () => undefined);
  await act(async () => undefined);
}

async function openAndAskRemove(onRemove: () => Promise<ErrorSentence | null>): Promise<void> {
  act(() => root.render(<ProjectMenu projectName="devboule-v2" onRemove={onRemove} />));
  await act(async () => kebab().click());
  await act(async () => {
    const entry = container.querySelector<HTMLButtonElement>("[role='menuitem']");
    entry?.click();
  });
}

describe("a project's kebab", () => {
  it("names its project and offers the one entry: Remove project", async () => {
    act(() =>
      root.render(<ProjectMenu projectName="devboule-v2" onRemove={vi.fn(async () => null)} />),
    );
    expect(kebab().getAttribute("aria-label")).toBe("Actions for devboule-v2");

    await act(async () => kebab().click());

    expect(menuItems()).toEqual(["Remove project"]);
  });

  it("asks first, keeps the folder in the words, and removes once on confirm", async () => {
    const onRemove = vi.fn(async () => null);
    await openAndAskRemove(onRemove);

    expect(dialog().textContent).toContain("stay on disk");
    expect(onRemove, "nothing is removed before the answer").not.toHaveBeenCalled();

    await act(async () => dialogButton("Remove").click());
    await settle();

    expect(onRemove).toHaveBeenCalledTimes(1);
    expect(document.querySelector("[role='dialog'], [role='alertdialog']")).toBeNull();
  });

  it("Cancel removes nothing", async () => {
    const onRemove = vi.fn(async () => null);
    await openAndAskRemove(onRemove);

    await act(async () => dialogButton("Cancel").click());
    await settle();

    expect(onRemove).not.toHaveBeenCalled();
  });

  it("two confirms in the same tick issue one removal", async () => {
    const onRemove = vi.fn(async () => null);
    await openAndAskRemove(onRemove);

    const confirm = dialogButton("Remove");
    await act(async () => {
      confirm.click();
      confirm.click();
    });
    await settle();

    expect(onRemove).toHaveBeenCalledTimes(1);
  });

  it("shows a refusal as its own sentence and keeps the project listed", async () => {
    const onRemove = vi.fn(async (): Promise<ErrorSentence | null> => ({
      sentence: "Stop the agents and terminals in this workspace first.",
      detail: null,
    }));
    await openAndAskRemove(onRemove);
    await act(async () => dialogButton("Remove").click());
    await settle();

    const alert = container.querySelector("[role='alert']");
    expect(alert?.textContent).toContain("Stop the agents and terminals in this workspace first.");
    expect(container.querySelector("button.sidebar-kebab")).not.toBeNull();
  });
});
