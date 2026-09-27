import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { moveMenuFocus } from "../strip/menuNav";
import type { TabMenuEntry } from "../strip/tabCloseMenu";
import { middleTruncate, type PaneHeaderMenu } from "./paneHeaderMenu";

export function PaneHeaderKebab({ menu }: { menu: PaneHeaderMenu }) {
  const [open, setOpen] = useState(false);
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");
  const kebabRef = useRef<HTMLButtonElement | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (copyTimer.current !== null) clearTimeout(copyTimer.current);
    },
    [],
  );

  // Focus the first entry that can act, as the strip's tab menu does: a
  // disabled row takes no focus.
  useEffect(() => {
    if (!open) return;
    const first = [...(listRef.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find(
      (button) => !button.disabled,
    );
    first?.focus({ preventScroll: true });
  }, [open]);

  // Outside press closes; the kebab counts as outside here too, but its own
  // click toggles, so a press on it never strands the menu open.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return;
      if (rootRef.current?.contains(event.target)) return;
      if (kebabRef.current?.contains(event.target)) return;
      setOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [open]);

  // Open over a viewport that then moved is stale: close it, handing focus
  // back to the kebab only when the menu had it — the strip's tab menu
  // contract, so the two menus cannot drift apart.
  useEffect(() => {
    if (!open) return;
    const onResize = () => {
      if (listRef.current?.contains(document.activeElement) === true) {
        kebabRef.current?.focus({ preventScroll: true });
      }
      setOpen(false);
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [open]);

  function closeToKebab() {
    kebabRef.current?.focus({ preventScroll: true });
    setOpen(false);
  }

  function onKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      closeToKebab();
      return;
    }
    if (event.key === "Tab") {
      // The menu floats over the transcript: continuing from here would walk
      // into the conversation. Hand focus back to the kebab instead.
      event.preventDefault();
      closeToKebab();
      return;
    }
    moveMenuFocus(listRef.current, event);
  }

  async function copyPath() {
    if (menu.copyPath === null) return;
    // `navigator.clipboard` is typed as always present but is not: a
    // non-secure context leaves it undefined, and awaiting undefined would
    // succeed and claim a copy that never happened.
    const clipboard: Clipboard | undefined = navigator.clipboard;
    if (clipboard === undefined) {
      setCopyState("failed");
      return;
    }
    try {
      await clipboard.writeText(menu.copyPath);
      setCopyState("copied");
    } catch {
      setCopyState("failed");
    }
    if (copyTimer.current !== null) clearTimeout(copyTimer.current);
    copyTimer.current = setTimeout(() => setCopyState("idle"), 1500);
  }

  function activateClose(key: TabMenuEntry["key"]) {
    menu.onCloseEntry?.(key);
    closeToKebab();
  }

  // The header's exact close set, whatever the seam carries: the tab menu's
  // rows arrive with left and delete among them, and neither belongs here —
  // the brief lists right, others and close, and delete destroys.
  const closes = menu.closeEntries.filter(
    (entry) => entry.key === "right" || entry.key === "others" || entry.key === "close",
  );

  return (
    <>
      <button
        ref={kebabRef}
        type="button"
        className="pane-header-kebab"
        aria-label="Session actions"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        ⋮
      </button>
      {open ? (
        <div ref={rootRef} className="pane-header-menu workspace-surface-menu">
          {menu.copyPath !== null ? (
            <div className="pane-header-path" title={menu.copyPath}>
              {middleTruncate(menu.copyPath)}
            </div>
          ) : null}
          <div ref={listRef} role="menu" aria-label="Session actions" onKeyDown={onKeyDown}>
            {menu.copyPath !== null ? (
              <button
                type="button"
                role="menuitem"
                className="workspace-surface-option"
                onClick={() => void copyPath()}
              >
                {copyState === "copied"
                  ? "Copied"
                  : copyState === "failed"
                    ? "Copy failed"
                    : "Copy path"}
              </button>
            ) : null}
            {menu.copyPath !== null ? (
              <div className="workspace-menu-separator" role="separator" />
            ) : null}
            {closes.map((entry) => (
              <button
                key={entry.key}
                type="button"
                role="menuitem"
                className="workspace-surface-option"
                disabled={entry.disabled || menu.onCloseEntry === null}
                onClick={() => activateClose(entry.key)}
              >
                {entry.label}
              </button>
            ))}
          </div>
        </div>
      ) : null}
    </>
  );
}
