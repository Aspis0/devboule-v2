import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { ErrorText } from "../../../components/ErrorText";
import type { ErrorSentence } from "../../../lib/errorSentence";
import { firstGrapheme } from "../../../lib/graphemeBound";
import { isImeComposition } from "../../../lib/imeComposition";
import { useMenuOpen } from "../../../lib/menuOpen";
import { validateWorkspaceTitle } from "../../../lib/workspaceTitles";
import { moveMenuFocus } from "../strip/menuNav";
import type { WorkspaceView } from "../workspaceProjects";
import { avatarStyle } from "./avatars";
import type { WorkspaceStat } from "./useWorkspaceStats";

/** The row's trailing state dot in the tab chips' vocabulary. */
const DOT_LABELS: Record<NonNullable<WorkspaceView["stateDot"]>, string> = {
  pulse: "running",
  attention: "needs attention",
  unattended: "running unattended",
};

export interface WorkspaceRowProps {
  workspace: WorkspaceView;
  projectName: string;
  selected: boolean;
  stat: WorkspaceStat | undefined;
  onSelect: (workspaceId: string) => void;
  /** Persists a new title and answers with the refusal, if one came back. */
  onRename: (workspaceId: string, title: string) => Promise<ErrorSentence | null>;
}

export function WorkspaceRow({
  workspace,
  projectName,
  selected,
  stat,
  onSelect,
  onRename,
}: WorkspaceRowProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(workspace.displayTitle);
  const [refusal, setRefusal] = useState<ErrorSentence | null>(null);
  const [saving, setSaving] = useState(false);
  const rowRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  /** Whether the row takes focus when the editor closes; a blur withdraws it. */
  const returnFocusRef = useRef(false);

  const closeMenu = useCallback((returnFocus: boolean) => {
    setMenuOpen(false);
    if (returnFocus) rowRef.current?.focus({ preventScroll: true });
  }, []);
  const dismissMenu = useCallback(() => setMenuOpen(false), []);
  useMenuOpen(menuOpen, dismissMenu);

  // Focus the menu's own entry: a menu nobody can reach with the keyboard is
  // a menu only the pointer can use.
  useEffect(() => {
    if (!menuOpen) return;
    const first = [...(menuRef.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find(
      (button) => !button.disabled,
    );
    first?.focus({ preventScroll: true });
  }, [menuOpen]);

  useEffect(() => {
    if (!menuOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return;
      if (menuRef.current?.contains(event.target)) return;
      setMenuOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [menuOpen]);

  // The editor opens selected, so typing replaces the title it is titled
  // with instead of extending it.
  useEffect(() => {
    if (!editing) return;
    inputRef.current?.focus({ preventScroll: true });
    inputRef.current?.select();
  }, [editing]);

  // Focus comes back to the row only once the button is mounted again — and
  // only from a keyboard close: a blur close leaves focus where it went.
  useLayoutEffect(() => {
    if (editing || !returnFocusRef.current) return;
    returnFocusRef.current = false;
    rowRef.current?.focus({ preventScroll: true });
  }, [editing]);

  const openMenu = (event: { preventDefault: () => void }) => {
    event.preventDefault();
    setMenuOpen(true);
  };

  const onRowKeyDown = (event: ReactKeyboardEvent<HTMLButtonElement>) => {
    if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) openMenu(event);
  };

  const onMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      closeMenu(true);
      return;
    }
    if (event.key === "Tab") {
      event.preventDefault();
      closeMenu(true);
      return;
    }
    moveMenuFocus(menuRef.current, event);
  };

  const startRename = () => {
    setDraft(workspace.displayTitle);
    setRefusal(null);
    closeMenu(false);
    setEditing(true);
  };

  const closeEditor = () => {
    setRefusal(null);
    setEditing(false);
  };

  const saveRename = async () => {
    if (saving) return;
    const clientRefusal = validateWorkspaceTitle(draft);
    if (clientRefusal !== null) {
      setRefusal({ sentence: clientRefusal, detail: null });
      return;
    }
    setSaving(true);
    const error = await onRename(workspace.id, draft.trim()).finally(() => setSaving(false));
    if (error !== null) {
      setRefusal(error);
      return;
    }
    closeEditor();
  };

  const onInputKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (isImeComposition(event.nativeEvent)) return;
    if (event.key === "Enter") {
      event.preventDefault();
      returnFocusRef.current = true;
      void saveRename();
      return;
    }
    if (event.key === "Escape") {
      if (saving) return;
      event.preventDefault();
      returnFocusRef.current = true;
      closeEditor();
    }
  };

  const onInputBlur = () => {
    // A blur withdraws the keyboard close's focus intent, and a save already
    // in flight owns the editor until it settles.
    returnFocusRef.current = false;
    if (saving) return;
    const trimmed = draft.trim();
    if (trimmed === "" || trimmed === workspace.displayTitle.trim()) {
      closeEditor();
      return;
    }
    void saveRename();
  };

  const dot = workspace.stateDot !== null ? `, ${DOT_LABELS[workspace.stateDot]}` : null;
  return (
    <div className="workspace-row-wrap" onContextMenu={openMenu}>
      {editing ? (
        <div className="workspace-row workspace-row-editing">
          <input
            ref={inputRef}
            aria-label="Workspace title"
            aria-busy={saving}
            readOnly={saving}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={onInputKeyDown}
            onBlur={onInputBlur}
          />
          {refusal !== null ? (
            <span className="workspace-row-rename-refusal" role="alert">
              <ErrorText
                sentence={refusal.sentence}
                detail={refusal.detail}
                id={`workspace-rename-refusal-${workspace.id}`}
              />
            </span>
          ) : null}
        </div>
      ) : (
        <button
          type="button"
          ref={rowRef}
          className={`workspace-row${selected ? " workspace-row-selected" : ""}`}
          onClick={() => onSelect(workspace.id)}
          aria-pressed={selected}
          aria-label={`${workspace.displayTitle}, ${projectName}${dot ?? ""}`}
          title={workspace.path ? workspace.path : undefined}
          onKeyDown={onRowKeyDown}
        >
          <span
            className="sidebar-avatar sidebar-avatar-workspace"
            style={avatarStyle(workspace.id)}
            aria-hidden="true"
          >
            {firstGrapheme(workspace.displayTitle)}
          </span>
          <span className="workspace-row-copy">
            <span className="workspace-row-title">{workspace.displayTitle}</span>
            {workspace.meta !== null ? (
              <span className="workspace-row-meta">{workspace.meta}</span>
            ) : null}
          </span>
          {stat !== undefined ? (
            <span className="sidebar-row-stats">
              <span className="sidebar-stat-add">+{stat.additions}</span>{" "}
              <span className="sidebar-stat-del">−{stat.deletions}</span>
            </span>
          ) : null}
          {workspace.stateDot !== null ? (
            <span
              role="img"
              aria-label={DOT_LABELS[workspace.stateDot]}
              className={`sidebar-row-dot sidebar-row-dot-${workspace.stateDot}${
                workspace.stateDot === "pulse" ? " dot-pulse" : ""
              }`}
            />
          ) : null}
        </button>
      )}
      {menuOpen ? (
        <div
          className="workspace-row-menu"
          ref={menuRef}
          role="menu"
          aria-label={`${workspace.displayTitle} actions`}
          onKeyDown={onMenuKeyDown}
        >
          <button type="button" role="menuitem" onClick={startRename}>
            Rename
          </button>
        </div>
      ) : null}
    </div>
  );
}
