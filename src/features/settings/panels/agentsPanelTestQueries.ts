// DOM lookups and form actions for the Agents panel test files: the profile
// list, the edit dialog and the new-profile form.

import { act } from "react";

import { dom, typeText } from "./agentsPanelTestHarness";

export function profileRows(): HTMLElement[] {
  return Array.from(dom.container.querySelectorAll<HTMLElement>(".agent-profile-row"));
}

export function rowByName(name: string): HTMLElement {
  // Keyed on the name element, not the row's whole text: the row carries
  // user prose (spawn prompt, note) that may name another profile.
  const row = profileRows().find(
    (candidate) => candidate.querySelector(".profile-name")?.textContent === name,
  );
  if (!row) throw new Error(`profile row ${name} did not render`);
  return row;
}

export function rowButton(name: string, text: string): HTMLButtonElement {
  const row = rowByName(name);
  const byLabel = row.querySelector<HTMLButtonElement>(`button[aria-label="${text} ${name}"]`);
  if (byLabel) return byLabel;
  const button = Array.from(row.querySelectorAll<HTMLButtonElement>("button")).find(
    (candidate) => candidate.textContent === text,
  );
  if (!button) throw new Error(`button ${text} on ${name} did not render`);
  return button;
}

/** The "+" action on the Agent profiles section label. */
export function newProfileButton(): HTMLButtonElement {
  const button = dom.container.querySelector<HTMLButtonElement>(
    '[data-settings-section] button[aria-label="New profile"]',
  );
  if (!button) throw new Error("New profile action did not render");
  return button;
}

/** A button anywhere in the Agents tab: the dialog renders outside the list. */
export function panelButton(text: string): HTMLButtonElement {
  const button = Array.from(
    dom.container.querySelectorAll<HTMLButtonElement>("#settings-panel-agents button"),
  ).find((candidate) => candidate.textContent === text);
  if (!button) throw new Error(`button ${text} did not render`);
  return button;
}

/** A button inside the open profile dialog, by its text. */
export function dialogButton(text: string): HTMLButtonElement {
  const button = Array.from(
    dom.container.querySelectorAll<HTMLButtonElement>(".edit-card button"),
  ).find((candidate) => candidate.textContent === text);
  if (!button) throw new Error(`dialog button ${text} did not render`);
  return button;
}

export async function openForm() {
  // The "+" action on the Agent profiles section label.
  const button = dom.container.querySelector<HTMLButtonElement>(
    '[data-settings-section] button[aria-label="New profile"]',
  );
  if (!button) throw new Error("New profile button did not render");
  await act(async () => button.click());
  await act(async () => undefined);
}

/**
 * Opens one stored row's editor. The form is shared with the New-profile
 * flow, so the editor is told apart by the class only the create mode carries.
 */
export async function openRowEditor(name: string): Promise<HTMLElement> {
  const edit = rowByName(name).querySelector<HTMLButtonElement>(
    `button[aria-label="Edit ${name}"]`,
  );
  if (!edit) throw new Error(`Edit button on ${name} did not render`);
  await act(async () => edit.click());
  await act(async () => undefined);
  const editor = dom.container.querySelector<HTMLElement>(
    ".agent-inline-editor:not(.agent-profile-create)",
  );
  if (!editor) throw new Error(`the editor of ${name} did not render`);
  return editor;
}

export function form(): HTMLElement {
  const element = dom.container.querySelector<HTMLElement>(".agent-profile-create");
  if (!element) throw new Error("new-profile form did not render");
  return element;
}

export function field<T extends Element>(selector: string): T {
  const element = form().querySelector<T>(selector);
  if (!element) throw new Error(`field ${selector} did not render in the form`);
  return element;
}

export function nameField(): HTMLInputElement {
  return field<HTMLInputElement>('input[aria-label="Profile name"]');
}

export function noteField(): HTMLTextAreaElement {
  return field<HTMLTextAreaElement>('textarea[aria-label="Profile note"]');
}

export function providerField(): HTMLSelectElement {
  return field<HTMLSelectElement>('select[aria-label="Provider"]');
}

/** The model control is a select when the provider published, input otherwise. */
export function modelControl(): HTMLInputElement | HTMLSelectElement {
  return field<HTMLInputElement | HTMLSelectElement>('[aria-label="Model"]');
}

export function modeControl(): HTMLInputElement | HTMLSelectElement {
  return field<HTMLInputElement | HTMLSelectElement>('[aria-label="Mode"]');
}

export function createButton(): HTMLButtonElement {
  const button = Array.from(form().querySelectorAll<HTMLButtonElement>("button")).find(
    (candidate) => candidate.textContent === "Create profile",
  );
  if (!button) throw new Error("Create profile button did not render");
  return button;
}

/** Select options' values, in wire order. */
export function selectValues(control: HTMLInputElement | HTMLSelectElement): string[] {
  if (control.tagName !== "SELECT") throw new Error(`control is a ${control.tagName}`);
  return Array.from((control as HTMLSelectElement).options).map((option) => option.value);
}

/** A fresh form draft filled for an older daemon: enough to save. */
export async function fillDraft() {
  await typeText(nameField(), "Gamma");
  await typeText(modelControl(), "claude-sonnet-4-5");
  await typeText(modeControl(), "default");
}
