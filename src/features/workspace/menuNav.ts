// Why: the strip's two menus share one keyboard model — arrows, Home and End
// among the ENABLED entries — extracted so the tab menu cannot drift from
// the "+" menu's behaviour, and so neither copies it again.

import type { KeyboardEvent as ReactKeyboardEvent } from "react";

export function moveMenuFocus(root: HTMLElement | null, event: ReactKeyboardEvent): void {
  if (
    event.key !== "ArrowDown" &&
    event.key !== "ArrowUp" &&
    event.key !== "Home" &&
    event.key !== "End"
  ) {
    return;
  }
  const enabled = [
    ...(root?.querySelectorAll<HTMLButtonElement>("[role='menuitem']") ?? []),
  ].filter((item) => !item.disabled);
  if (enabled.length === 0) return;
  event.preventDefault();
  const current = enabled.indexOf(document.activeElement as HTMLButtonElement);
  if (event.key === "Home") {
    enabled[0].focus({ preventScroll: true });
    return;
  }
  if (event.key === "End") {
    enabled[enabled.length - 1].focus({ preventScroll: true });
    return;
  }
  if (current === -1) {
    enabled[event.key === "ArrowDown" ? 0 : enabled.length - 1].focus({ preventScroll: true });
    return;
  }
  if (enabled.length === 1) return;
  const next =
    enabled[(current + (event.key === "ArrowDown" ? 1 : -1) + enabled.length) % enabled.length];
  next.focus({ preventScroll: true });
}
