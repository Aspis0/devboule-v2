import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { ConfirmDialog } from "../../../components/ConfirmDialog";
import { ErrorText } from "../../../components/ErrorText";
import type { ErrorSentence } from "../../../lib/errorSentence";
import { useMenuOpen } from "../../../lib/menuOpen";
import { moveMenuFocus } from "../strip/menuNav";

export interface ProjectMenuProps {
  projectName: string;
  /** Takes the project off the list and answers with the refusal, if one came back. */
  onRemove: () => Promise<ErrorSentence | null>;
}

/**
 * A project's kebab: one entry, Remove project. It asks first and takes the
 * project off the list; the folder stays on disk and its workspaces are archived.
 */
export function ProjectMenu({ projectName, onRemove }: ProjectMenuProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [refusal, setRefusal] = useState<ErrorSentence | null>(null);
  const kebabRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const removingRef = useRef(false);
  const refusalId = useId();

  const dismissMenu = useCallback(() => setMenuOpen(false), []);
  useMenuOpen(menuOpen, dismissMenu);

  // The menu's own entry takes the focus: a menu only the pointer can reach is
  // a menu the keyboard cannot use.
  useEffect(() => {
    if (!menuOpen) return;
    menuRef.current?.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
  }, [menuOpen]);

  useEffect(() => {
    if (!menuOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return;
      if (menuRef.current?.contains(event.target)) return;
      if (kebabRef.current?.contains(event.target)) return;
      setMenuOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [menuOpen]);

  const onMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape" || event.key === "Tab") {
      event.preventDefault();
      setMenuOpen(false);
      kebabRef.current?.focus({ preventScroll: true });
      return;
    }
    moveMenuFocus(menuRef.current, event);
  };

  const confirmRemove = async () => {
    // Set synchronously: two confirms in one tick must issue one removal.
    if (removingRef.current) return;
    removingRef.current = true;
    setConfirming(false);
    const error = await onRemove();
    removingRef.current = false;
    if (error !== null) setRefusal(error);
  };

  return (
    <span className="sidebar-menu-anchor">
      <button
        type="button"
        ref={kebabRef}
        className="sidebar-kebab"
        aria-label={`Actions for ${projectName}`}
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        onClick={() => setMenuOpen(!menuOpen)}
      >
        <span aria-hidden="true">⋯</span>
      </button>
      {menuOpen ? (
        <div
          className="sidebar-menu"
          ref={menuRef}
          role="menu"
          aria-label={`${projectName} actions`}
          onKeyDown={onMenuKeyDown}
        >
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setMenuOpen(false);
              setRefusal(null);
              setConfirming(true);
            }}
          >
            Remove project
          </button>
        </div>
      ) : null}
      {refusal !== null ? (
        <div className="workspace-project-error" role="alert">
          <ErrorText sentence={refusal.sentence} detail={refusal.detail} id={refusalId} />
        </div>
      ) : null}
      <ConfirmDialog
        open={confirming}
        title={`Remove “${projectName}”?`}
        message="Takes the project and its workspaces off this list. The folders stay on disk."
        confirmLabel="Remove"
        tone="accent"
        onConfirm={() => void confirmRemove()}
        onCancel={() => setConfirming(false)}
      />
    </span>
  );
}
