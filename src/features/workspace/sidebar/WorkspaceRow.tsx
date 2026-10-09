import {
  memo,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { ErrorText } from "../../../components/ErrorText";
import { ConfirmDialog } from "../../../components/ConfirmDialog";
import type { ErrorSentence } from "../../../lib/errorSentence";
import { firstGrapheme } from "../../../lib/graphemeBound";
import { isImeComposition } from "../../../lib/imeComposition";
import { useMenuOpen } from "../../../lib/menuOpen";
import { validateWorkspaceTitle } from "../../../lib/workspaceTitles";
import { moveMenuFocus } from "../strip/menuNav";
import type { WorkspaceView } from "../workspaceProjects";
import type { WorkspaceKey } from "../hosts/hostIdentity";
import { avatarStyle } from "./avatars";
import { rowFact } from "./rowFact";
import type { WorkspaceStat } from "./useWorkspaceStats";

/**
 * Where focus goes once this row's workspace is gone: the next row, the
 * previous one, or the project's own create control — never the body.
 */
function focusTargetAfterRemoval(row: HTMLButtonElement): HTMLElement | null {
  const scope: ParentNode = row.closest(".workspace-project-items") ?? document;
  const rows = [...scope.querySelectorAll<HTMLButtonElement>("button.workspace-row")];
  const index = rows.indexOf(row);
  return (
    rows[index + 1] ??
    rows[index - 1] ??
    row.closest(".workspace-project")?.querySelector<HTMLButtonElement>(".workspace-project-new") ??
    null
  );
}

/**
 * A search that matched only the deleted row removes its whole project group,
 * and the captured target with it: fall back to whatever row the tree still
 * shows, else the search field.
 */
function focusSurvivor(target: HTMLElement | null, panel: Element | null): void {
  const survivor = target?.isConnected
    ? target
    : (panel?.querySelector<HTMLElement>("button.workspace-row") ??
      panel?.querySelector<HTMLElement>(".workspace-search input"));
  survivor?.focus({ preventScroll: true });
}

/**
 * A workspace named like its project prints the branch instead: the heading
 * above already said this name, and a row never says it twice. With no branch
 * known the row prints its host — the only other short truth about it.
 */
function rowLabel(
  workspace: WorkspaceView,
  projectName: string,
  branch: string | undefined,
  hostName: string,
): string {
  if (workspace.displayTitle !== projectName) return workspace.displayTitle;
  return branch ?? hostName;
}

/**
 * What a tooltip carries for a row: where the workspace lives, what branch it
 * sits on and what is uncommitted — the facts the row itself does not print.
 */
function rowDetail(
  workspace: WorkspaceView,
  branch: string | undefined,
  stat: WorkspaceStat | undefined,
): string | undefined {
  const totals =
    stat !== undefined && stat.additions + stat.deletions > 0
      ? `+${stat.additions} −${stat.deletions}`
      : null;
  const parts = [workspace.path, branch, totals].filter((part): part is string => Boolean(part));
  return parts.length > 0 ? parts.join(" · ") : undefined;
}

export interface WorkspaceRowProps {
  workspace: WorkspaceView;
  /** The row's identity in the UI; null only when the daemon's id is blank,
   * and a row the UI cannot name cannot be selected. */
  workspaceKey: WorkspaceKey | null;
  projectName: string;
  /** The row's host as a short label; the second line, and the title's
   * fallback when the project heading already said the workspace's name. */
  hostName: string;
  selected: boolean;
  /** An agent listed under this row is the tab in front: it carries the marker. */
  agentFocused: boolean;
  stat: WorkspaceStat | undefined;
  /** The branch the workspace's last status read reported; it also speaks the
   *  row when the project's name above already said this workspace's name. */
  branch: string | undefined;
  onSelect: (workspaceKey: WorkspaceKey) => void;
  /** Persists a new title and answers with the refusal, if one came back. */
  onRename: (workspaceId: string, title: string) => Promise<ErrorSentence | null>;
  /** Deletes the workspace and answers with the refusal, if one came back. */
  onDelete: (workspaceId: string) => Promise<ErrorSentence | null>;
}

/**
 * One workspace row. Memoised: the daemon republishes every two seconds and
 * Workspace re-renders with it, so a row whose own facts did not change must
 * not be re-rendered — the props above are all stable references or values.
 */
export const WorkspaceRow = memo(function WorkspaceRow({
  workspace,
  workspaceKey,
  projectName,
  hostName,
  selected,
  agentFocused,
  stat,
  branch,
  onSelect,
  onRename,
  onDelete,
}: WorkspaceRowProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(workspace.displayTitle);
  const [refusal, setRefusal] = useState<ErrorSentence | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [deleteRefusal, setDeleteRefusal] = useState<ErrorSentence | null>(null);
  // `title` is the pointer's channel alone: the sentence rides the button as a
  // description a screen reader announces. An id of its own, since two hosts
  // may hold the same workspace id.
  const detailId = useId();
  const rowRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  /** Whether the row takes focus when the editor closes; a blur withdraws it. */
  const returnFocusRef = useRef(false);
  /** Set when the delete ask opens: the ask's own restore aims at the menu
   * entry its open unmounted, so the row takes focus back when it ends. */
  const deleteAskPendingRef = useRef(false);
  /** Set synchronously by the confirm: state would not be committed before a
   * second confirm in the same tick reached the handler. */
  const deletePendingRef = useRef(false);
  /** Set by a successful delete: the re-read commits after the answer, so
   * focus can only be placed once this row has actually unmounted. */
  const focusAfterRemovalRef = useRef<(() => void) | null>(null);

  useLayoutEffect(
    () => () => {
      const focus = focusAfterRemovalRef.current;
      if (focus !== null) queueMicrotask(focus);
    },
    [],
  );

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

  useEffect(() => {
    if (confirming || !deleteAskPendingRef.current) return;
    deleteAskPendingRef.current = false;
    rowRef.current?.focus({ preventScroll: true });
  }, [confirming]);

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

  const startDelete = () => {
    if (deletePendingRef.current) return;
    focusAfterRemovalRef.current = null;
    setDeleteRefusal(null);
    closeMenu(false);
    deleteAskPendingRef.current = true;
    setConfirming(true);
  };

  const confirmDelete = async () => {
    if (deletePendingRef.current) return;
    deletePendingRef.current = true;
    const row = rowRef.current;
    const target = row === null ? null : focusTargetAfterRemoval(row);
    const panel = row?.closest(".workspace-panel-open") ?? null;
    setConfirming(false);
    const error = await onDelete(workspace.id);
    deletePendingRef.current = false;
    if (error !== null) {
      setDeleteRefusal(error);
      return;
    }
    const focus = () => focusSurvivor(target, panel);
    if (row?.isConnected) focusAfterRemovalRef.current = focus;
    else focus();
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

  const fact = rowFact(workspace);
  const label = rowLabel(workspace, projectName, branch, hostName);
  // The second line: the branch in mono unless it already is the title, and
  // the host with its glyph unless the title already said it. What is
  // uncommitted rides on the right.
  const subBranch = branch !== undefined && branch !== label ? branch : null;
  const subHost = hostName !== label ? hostName : null;
  const totals =
    stat !== undefined && stat.additions + stat.deletions > 0
      ? `+${stat.additions} −${stat.deletions}`
      : null;
  const twoLines = subBranch !== null || subHost !== null || totals !== null;
  // The visible label leads the name; a branch or host standing in for the
  // title is followed by the title, so the row stays findable by what it is
  // called — and the name says each thing once, never the project's name twice.
  const nameParts = [label];
  if (label !== workspace.displayTitle) nameParts.push(workspace.displayTitle);
  if (!nameParts.includes(projectName)) nameParts.push(projectName);
  if (subHost !== null && !nameParts.includes(hostName)) nameParts.push(hostName);
  if (fact !== null) nameParts.push(...fact.parts.map((part) => part.text));
  const ariaLabel = nameParts.join(", ");
  const detail = rowDetail(workspace, branch, stat);
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
        <>
          <button
            type="button"
            ref={rowRef}
            className={`workspace-row${selected ? " workspace-row-selected" : ""}${
              agentFocused ? " workspace-row-agent-focused" : ""
            }${twoLines ? " workspace-row-two" : ""}`}
            onClick={() => {
              if (workspaceKey !== null) onSelect(workspaceKey);
            }}
            aria-pressed={selected}
            aria-label={ariaLabel}
            aria-describedby={detail === undefined ? undefined : detailId}
            title={detail}
            onKeyDown={onRowKeyDown}
          >
            <span
              className="sidebar-avatar sidebar-avatar-workspace"
              style={avatarStyle(workspace.id)}
              aria-hidden="true"
            >
              {firstGrapheme(workspace.displayTitle)}
            </span>
            <span className="workspace-row-body">
              <span className="workspace-row-line">
                <span className="workspace-row-title">{label}</span>
                {fact === null ? null : (
                  <span className="workspace-row-fact">
                    {fact.dot === null ? null : (
                      <span
                        aria-hidden="true"
                        className={`sidebar-row-dot sidebar-row-dot-${fact.dot}${
                          fact.dot === "pulse" ? " dot-pulse" : ""
                        }`}
                      />
                    )}
                    {fact.parts.map((part, index) => (
                      <span
                        key={part.text}
                        className={part.attention ? "sidebar-row-waiting" : undefined}
                      >
                        {/* A flex item trims its own leading space, so the space
                          before the dot cannot be an ordinary one. */}
                        {index > 0 ? " · " : ""}
                        {part.text}
                      </span>
                    ))}
                  </span>
                )}
              </span>
              {twoLines ? (
                <span className="workspace-row-line workspace-row-sub">
                  {subBranch === null ? null : (
                    <span className="workspace-row-branch">{subBranch}</span>
                  )}
                  {subHost === null ? null : (
                    <span className="workspace-row-host">
                      <svg
                        className="workspace-row-host-glyph"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="1.75"
                        strokeLinecap="round"
                        aria-hidden="true"
                        focusable="false"
                      >
                        <rect x="3" y="4" width="18" height="7" rx="1.5" />
                        <rect x="3" y="13" width="18" height="7" rx="1.5" />
                        <circle cx="7" cy="7.5" r="1" fill="currentColor" stroke="none" />
                        <circle cx="7" cy="16.5" r="1" fill="currentColor" stroke="none" />
                      </svg>
                      {subHost}
                    </span>
                  )}
                  {totals === null ? null : <span className="workspace-row-totals">{totals}</span>}
                </span>
              ) : null}
            </span>
          </button>
          {detail === undefined ? null : (
            <span id={detailId} className="sr-only">
              {detail}
            </span>
          )}
        </>
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
          {/* A local row is the project folder and the daemon refuses its
              delete outright (session_workspaces.rs) — never an affordance. */}
          {workspace.isolation === "worktree" ? (
            <button type="button" role="menuitem" onClick={startDelete}>
              Delete workspace
            </button>
          ) : null}
        </div>
      ) : null}
      {deleteRefusal !== null ? (
        // The mapped sentence only: no raw daemon text reaches this surface.
        <span className="workspace-row-delete-refusal" role="alert">
          {deleteRefusal.sentence}
        </span>
      ) : null}
      <ConfirmDialog
        open={confirming}
        title={`Delete “${workspace.displayTitle}”?`}
        message="Deletes the worktree folder and this workspace's entry. Your project folder is kept. This cannot be undone."
        confirmLabel="Delete"
        tone="danger"
        onConfirm={() => void confirmDelete()}
        onCancel={() => setConfirming(false)}
      />
    </div>
  );
});
