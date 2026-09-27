import { useEffect, useRef, useState } from "react";

/**
 * One row's kebab: Update (only when the row can update), Refresh, copy
 * path. Opens onto its first item, arrows travel, Escape closes and returns
 * focus to the button, choosing closes. Copy feedback stays on the item
 * until the menu closes — no timers, nothing to outlive the menu.
 */
export function ProviderKebab({
  providerId,
  path,
  onUpdate,
  onRefresh,
}: {
  providerId: string;
  /** The executable path the copy item writes. */
  path: string;
  /** Absent when the row cannot update: the item is omitted, not disabled. */
  onUpdate?: () => void;
  onRefresh: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [copyNote, setCopyNote] = useState<string | null>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (open) {
      setCopyNote(null);
      menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    }
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (
        target !== null &&
        !menuRef.current?.contains(target) &&
        !buttonRef.current?.contains(target)
      ) {
        setOpen(false);
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [open]);

  function close(returnFocus: boolean) {
    setOpen(false);
    if (returnFocus) buttonRef.current?.focus();
  }

  function onMenuKeyDown(event: React.KeyboardEvent) {
    const items = Array.from(
      menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [],
    );
    if (items.length === 0) return;
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Escape") {
      event.preventDefault();
      close(true);
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      items[(index + 1) % items.length]?.focus();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      items[(index - 1 + items.length) % items.length]?.focus();
    } else if (event.key === "Home") {
      event.preventDefault();
      items[0]?.focus();
    } else if (event.key === "End") {
      event.preventDefault();
      items[items.length - 1]?.focus();
    }
  }

  async function copyPath() {
    try {
      const clipboard = (
        navigator as Navigator & {
          clipboard?: { writeText: (text: string) => Promise<void> };
        }
      ).clipboard;
      if (!clipboard) throw new Error("no clipboard in this host");
      await clipboard.writeText(path);
      setCopyNote("Copied");
    } catch {
      setCopyNote("Copy failed");
    }
  }

  return (
    <span className="prov-kebab-wrap">
      <button
        ref={buttonRef}
        type="button"
        className="prov-kebab"
        aria-label={`Actions for ${providerId}`}
        aria-expanded={open}
        aria-haspopup="menu"
        onClick={() => (open ? close(false) : setOpen(true))}
      >
        <span aria-hidden="true">⋮</span>
      </button>
      {open ? (
        <div
          ref={menuRef}
          role="menu"
          aria-label={`Actions for ${providerId}`}
          className="prov-menu"
          onKeyDown={onMenuKeyDown}
        >
          {onUpdate ? (
            <button
              type="button"
              role="menuitem"
              className="prov-menu-item"
              onClick={() => {
                close(false);
                onUpdate();
              }}
            >
              Update
            </button>
          ) : null}
          <button
            type="button"
            role="menuitem"
            className="prov-menu-item"
            onClick={() => {
              close(false);
              onRefresh();
            }}
          >
            Refresh
          </button>
          <button
            type="button"
            role="menuitem"
            className="prov-menu-item"
            onClick={() => void copyPath()}
          >
            {copyNote === null ? "Copy path" : copyNote}
          </button>
        </div>
      ) : null}
    </span>
  );
}
