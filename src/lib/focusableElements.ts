/** The dialog's focusable elements, for the trap: every button, field and
 * link that can take focus and is neither disabled nor hidden. Shared by the
 * app's modal dialogs — a third copy of this query is a third place for the
 * selector to rot. */
export function getFocusableElements(container: HTMLElement): HTMLElement[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
    ),
  ).filter(
    (element) => !element.hasAttribute("hidden") && element.getAttribute("aria-hidden") !== "true",
  );
}
