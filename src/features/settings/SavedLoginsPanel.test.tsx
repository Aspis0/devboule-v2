// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/tauri")>();
  return {
    ...actual,
    isCommandError: vi.fn(
      (error: unknown) =>
        typeof error === "object" && error !== null && "code" in error && "message" in error,
    ),
    savedLoginsList: vi.fn(),
    savedLoginCreate: vi.fn(),
    savedLoginUpdate: vi.fn(),
    savedLoginDelete: vi.fn(),
  };
});

import {
  savedLoginCreate,
  savedLoginDelete,
  savedLoginsList,
  savedLoginUpdate,
} from "../../lib/tauri";
import type { SavedLogin } from "../../types/ipc";
import { SavedLoginsPanel } from "./SavedLoginsPanel";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** What one saved row is on screen. */
const WORK: SavedLogin = {
  id: "0123456789abcdef0123456789abcdef",
  label: "Work mail",
  origins: ["https://mail.example.test"],
  username: "person@example.test",
};

/** A second row, so a test can switch between two of them. */
const BANK: SavedLogin = {
  id: "fedcba9876543210fedcba9876543210",
  label: "Bank",
  origins: ["https://bank.example.test"],
  username: "person@bank.test",
};

/** A sentinel only this test knows, to prove the page never renders one back. */
const SECRET = "SENTINEL-PW-7f3a";

function type(input: HTMLInputElement | HTMLTextAreaElement, value: string): void {
  const set = Object.getOwnPropertyDescriptor(input.constructor.prototype, "value")?.set;
  if (set === undefined) throw new Error("no value setter on this control");
  set.call(input, value);
}

/** The mounted panel; the helpers below read it and the tests below mount it. */
let container!: HTMLDivElement;

async function press(label: string, within?: ParentNode): Promise<HTMLButtonElement> {
  const scope = within ?? container;
  const button = [...scope.querySelectorAll("button")].find(
    (one) => one.textContent === label || one.getAttribute("aria-label") === label,
  );
  if (button === undefined) throw new Error(`no button labelled ${label}`);
  await act(async () => button.click());
  return button;
}

async function field(name: string): Promise<HTMLInputElement | HTMLTextAreaElement> {
  const found = container.querySelector<HTMLInputElement | HTMLTextAreaElement>(
    `form [name="${name}"]`,
  );
  if (found === null) throw new Error(`the form has no ${name}`);
  return found;
}

async function fill(name: string, value: string): Promise<void> {
  const input = await field(name);
  await act(async () => {
    type(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function submit(): Promise<void> {
  const form = container.querySelector("form");
  if (form === null) throw new Error("no form is on screen");
  await act(async () => {
    form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  });
}

describe("Saved logins panel", () => {
  let root: Root;
  let listed: SavedLogin[];

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    listed = [WORK];
    vi.mocked(savedLoginsList).mockImplementation(async () => listed);
    // The commands answer metadata only: `SavedLogin` has no field a password
    // could come back in, and a mock that invented one would test nothing.
    vi.mocked(savedLoginCreate).mockImplementation(async (draft) => ({
      id: "1".repeat(32),
      label: draft.label,
      origins: draft.origins,
      username: draft.username,
    }));
    vi.mocked(savedLoginUpdate).mockImplementation(async (patch) => ({
      id: patch.id,
      label: patch.label,
      origins: patch.origins,
      username: patch.username,
    }));
    vi.mocked(savedLoginDelete).mockImplementation(async () => undefined);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  async function render(): Promise<void> {
    root = createRoot(container);
    await act(async () => root.render(<SavedLoginsPanel />));
    await act(async () => undefined);
  }

  it("keeps the confirmation region mounted, so a later message is announced", async () => {
    await render();
    const region = container.querySelector<HTMLElement>('p.settings-status[role="status"]');
    expect(region, "the status region must exist before any save").not.toBeNull();
    expect(region?.textContent).toBe("");
  });

  it("lists what the machine saved and shows no password field at all", async () => {
    await render();

    expect(container.textContent).toContain("Work mail");
    expect(container.textContent).toContain("https://mail.example.test");
    expect(container.textContent).toContain("person@example.test");
    expect(container.textContent).toContain("Password saved");
    // Nothing on this page holds a password to type: the list is metadata.
    expect(container.querySelector('input[type="password"]')).toBeNull();
  });

  it("says so when this machine has saved nothing", async () => {
    listed = [];
    await render();

    expect(container.textContent).toContain("No saved logins yet.");
    expect(container.querySelector('[aria-label="Add a login"]')).not.toBeNull();
  });

  it("shows the vault's own refusal, not the daemon's sentence for its code", async () => {
    vi.mocked(savedLoginsList).mockRejectedValue({
      code: "internal",
      message: "This machine's credential store refused: the store is locked.",
    });
    await render();

    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("This machine's credential store refused");
    expect(alert?.textContent).not.toContain("agent daemon");
  });

  it("saves what the person typed and takes the password out of the page", async () => {
    await render();

    await press("Add a login");
    await fill("label", "Bank");
    await fill("origins", "https://bank.example.test\nhttps://bank.example.test:8443");
    await fill("username", "person");
    await fill("password", SECRET);
    await submit();

    expect(savedLoginCreate).toHaveBeenCalledWith({
      label: "Bank",
      origins: ["https://bank.example.test", "https://bank.example.test:8443"],
      username: "person",
      password: SECRET,
    });
    expect(container.querySelector("form")).toBeNull();
    expect(container.innerHTML).not.toContain(SECRET);
  });

  it("keeps the stored password when the change form is left empty", async () => {
    await render();

    await press("Edit");
    const password = (await field("password")) as HTMLInputElement;
    expect(password.type).toBe("password");
    expect(password.value).toBe("");
    expect(container.textContent).toContain("Leave empty to keep the saved password.");
    await fill("label", "Work mail (personal)");
    await submit();

    const [patch] = vi.mocked(savedLoginUpdate).mock.calls[0]!;
    expect(patch).toMatchObject({ id: WORK.id, label: "Work mail (personal)" });
    expect(Object.hasOwn(patch, "password")).toBe(false);
  });

  it("deletes only after the confirm, and tells the person what happened", async () => {
    await render();

    await press("Delete");
    expect(savedLoginDelete).not.toHaveBeenCalled();
    await press("Cancel");
    expect(savedLoginDelete).not.toHaveBeenCalled();

    await press("Delete");
    await press("Delete now");

    expect(savedLoginDelete).toHaveBeenCalledWith(WORK.id);
    expect(container.textContent).toContain("Deleted Work mail.");
  });

  it("keeps a refused delete on screen with what the vault said", async () => {
    vi.mocked(savedLoginDelete).mockRejectedValue({
      code: "internal",
      message:
        "The password is gone from the credential store; delete the entry again to clear it from the list.",
    });
    await render();

    await press("Delete");
    await press("Delete now");

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "delete the entry again",
    );
    expect(container.textContent).toContain("Work mail");
  });

  it("empties the password field even when the save is refused", async () => {
    vi.mocked(savedLoginCreate).mockRejectedValue({
      code: "internal",
      message: "This machine's credential store refused: the store is locked.",
    });
    await render();

    await press("Add a login");
    await fill("label", "Bank");
    await fill("origins", "https://bank.example.test");
    await fill("password", SECRET);
    await submit();

    // The form stays open so the person can fix what was wrong and try again,
    // and what they typed is not sitting in the page while they do.
    expect(container.querySelector("form")).not.toBeNull();
    expect(((await field("password")) as HTMLInputElement).value).toBe("");
    expect(container.innerHTML).not.toContain(SECRET);
  });

  it("sends the row the form is on, not the row that was open first", async () => {
    listed = [WORK, BANK];
    await render();

    const rowOf = (label: string): Element => {
      const row = [...container.querySelectorAll(".saved-login-row")].find((one) =>
        one.textContent?.startsWith(label),
      );
      if (row === undefined) throw new Error(`no row for ${label}`);
      return row;
    };
    await press("Edit", rowOf("Work mail"));
    expect(((await field("label")) as HTMLInputElement).value).toBe(WORK.label);
    await fill("label", "Work mail (edited)");

    await press("Edit", rowOf("Bank"));

    // The form is the second row's now: its own values, and its own id.
    expect(((await field("label")) as HTMLInputElement).value).toBe(BANK.label);
    expect(((await field("username")) as HTMLInputElement).value).toBe(BANK.username);
    await submit();

    const [patch] = vi.mocked(savedLoginUpdate).mock.calls[0]!;
    expect(patch).toMatchObject({ id: BANK.id, label: BANK.label });
  });

  it("closes the form of the row it just deleted", async () => {
    await render();

    await press("Edit");
    expect(container.querySelector("form")).not.toBeNull();

    await press("Delete");
    await press("Delete now");

    expect(container.querySelector("form")).toBeNull();
    expect(container.textContent).toContain("Deleted Work mail.");
  });

  it("moves focus to the first field, and to the confirm it armed", async () => {
    await render();

    await press("Add a login");
    expect(document.activeElement).toBe(await field("label"));

    await press("Cancel");
    await press("Delete");
    expect(document.activeElement?.className).toContain("saved-login-confirm");
  });

  it("writes nothing once the page has gone", async () => {
    let answer = () => new Promise<SavedLogin>((resolve) => resolve(WORK));
    vi.mocked(savedLoginCreate).mockImplementation(() => answer());
    await render();

    await press("Add a login");
    await fill("label", "Bank");
    await fill("origins", "https://bank.example.test");
    await fill("password", SECRET);
    const settled = act(async () => {
      await submit();
    });
    await act(async () => root.unmount());
    await settled;
    container = document.createElement("div");
    document.body.appendChild(container);

    // Nothing to assert on the DOM: the point is that a promise that settled
    // after the unmount wrote nothing and threw nothing.
    expect(savedLoginCreate).toHaveBeenCalledTimes(1);
  });
});
