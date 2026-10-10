// Test helpers for the app select (AppSelect): read a field's options, or pick one,
// the way a person does, by opening its listbox. Shared by every suite that drives one.

import { act } from "react";

/** The options an app select offers, read by opening its listbox and closing it again. */
export async function selectOptions(
  control: HTMLElement,
): Promise<{ value: string; label: string }[]> {
  if (control.tagName !== "BUTTON") throw new Error(`control is a ${control.tagName}`);
  await act(async () => (control as HTMLButtonElement).click());
  const options = Array.from(
    control.parentElement?.querySelectorAll<HTMLElement>("[role='option']") ?? [],
  ).map((option) => ({ value: option.dataset.value ?? "", label: option.textContent ?? "" }));
  await act(async () => (control as HTMLButtonElement).click());
  return options;
}

/** The values an app select offers. */
export async function selectValues(control: HTMLElement): Promise<string[]> {
  return (await selectOptions(control)).map((option) => option.value);
}

/** Picks a value from an app select the way a person does: open it, then choose the row. */
export async function pickSelect(control: HTMLElement, value: string): Promise<void> {
  await act(async () => (control as HTMLButtonElement).click());
  const option = Array.from(
    control.parentElement?.querySelectorAll<HTMLButtonElement>("[role='option']") ?? [],
  ).find((candidate) => candidate.dataset.value === value);
  if (!option) throw new Error(`option ${value} did not render`);
  await act(async () => option.click());
}

/** The current value of a text field, or of an app select (its trigger carries it). */
export function valueOf(control: HTMLElement): string {
  if (control.tagName === "BUTTON") return (control as HTMLButtonElement).dataset.value ?? "";
  return (control as HTMLInputElement | HTMLTextAreaElement).value;
}
