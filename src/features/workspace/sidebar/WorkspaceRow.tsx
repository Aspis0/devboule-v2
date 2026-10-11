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
import { isImeComposition } from "../../../lib/imeComposition";
import { useMenuOpen } from "../../../lib/menuOpen";
import { validateWorkspaceTitle } from "../../../lib/workspaceTitles";
import { moveMenuFocus } from "../strip/menuNav";
import type { WorkspaceView } from "../workspaceProjects";
import type { HostId, WorkspaceKey } from "../hosts/hostIdentity";
import { compactLineCount } from "./diffCount";
import { rowStatus } from "./rowStatus";
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
 * What a tooltip carries for a row: its whole title, where the workspace lives,
 * what branch it sits on and what is uncommitted — what the row cuts or does
 * not print.
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
  // The title leads: a long one is cut in the row, and the tooltip is where it reads whole.
  const parts = [workspace.displayTitle, workspace.path, branch, totals].filter(
    (part): part is string => Boolean(part),
  );
  return parts.length > 0 ? parts.join(" · ") : undefined;
}

export interface WorkspaceRowProps {
  workspace: WorkspaceView;
  /** The row's identity in the UI; null only when the daemon's id is blank,
   * and a row the UI cannot name cannot be selected. */
  workspaceKey: WorkspaceKey | null;
  /** The row's host as a short label: the meta line of a remote row. */
  hostName: string;
  selected: boolean;
  stat: WorkspaceStat | undefined;
  /** The branch the workspace's last status read reported, for the meta line. */
  branch: string | undefined;
  onSelect: (workspaceKey: WorkspaceKey) => void;
  /** Persists a new title and answers with the refusal, if one came back. */
  onRename: (workspaceId: string, title: string) => Promise<ErrorSentence | null>;
  /** Takes the workspace off the list (its folder stays) and answers with the
   * refusal, if one came back. */
  onArchive: (workspaceId: string, hostId: HostId) => Promise<ErrorSentence | null>;
  /** A workspace on another machine: read-only here until host-targeted
   * mutations land, and its second line may carry the offline label. */
  remote?: boolean;
  /** Whether that host answers right now. */
  hostOffline?: boolean;
}

/** The one word a row on an unreachable host carries. */
const OFFLINE_LABEL = "offline";

/**
 * One workspace row. Memoised: the daemon republishes every two seconds and
 * Workspace re-renders with it, so a row whose own facts did not change must
 * not be re-rendered — the props above are all stable references or values.
 */
export const WorkspaceRow = memo(function WorkspaceRow({
  workspace,
  workspaceKey,
  hostName,
  selected,
  stat,
  branch,
  onSelect,
  onRename,
  onArchive,
  remote = false,
  hostOffline = false,
}: WorkspaceRowProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(workspace.displayTitle);
  const [refusal, setRefusal] = useState<ErrorSentence | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [archiveRefusal, setArchiveRefusal] = useState<ErrorSentence | null>(null);
  // `title` is the pointer's channel alone: the sentence rides the button as a
  // description a screen reader announces. An id of its own, since two hosts
  // may hold the same workspace id.
  const detailId = useId();
  const rowRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const kebabRef = useRef<HTMLButtonElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  /** Whether the row takes focus when the editor closes; a blur withdraws it. */
  const returnFocusRef = useRef(false);
  /** Set when the archive ask opens: the ask's own restore aims at the menu
   * entry its open unmounted, so the row takes focus back when it ends. */
  const archiveAskPendingRef = useRef(false);
  /** Set synchronously by the confirm: state would not be committed before a
   * second confirm in the same tick reached the handler. */
  const archivePendingRef = useRef(false);
  /** Set by a successful archive: the re-read commits after the answer, so
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
      // The kebab toggles its own menu: a press on it must not close it first.
      if (kebabRef.current?.contains(event.target)) return;
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
    if (confirming || !archiveAskPendingRef.current) return;
    archiveAskPendingRef.current = false;
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

  const startArchive = () => {
    if (archivePendingRef.current) return;
    focusAfterRemovalRef.current = null;
    setArchiveRefusal(null);
    closeMenu(false);
    archiveAskPendingRef.current = true;
    setConfirming(true);
  };

  const confirmArchive = async () => {
    if (archivePendingRef.current) return;
    archivePendingRef.current = true;
    const row = rowRef.current;
    const target = row === null ? null : focusTargetAfterRemoval(row);
    const panel = row?.closest(".workspace-panel-open") ?? null;
    setConfirming(false);
    const error = await onArchive(workspace.id, workspace.hostId);
    archivePendingRef.current = false;
    if (error !== null) {
      setArchiveRefusal(error);
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

  // The title is the workspace's own name, always: the host never stands in
  // for it. The meta line below it carries the branch, the remote host, and
  // what is uncommitted, and it exists only when one of those has something
  // to say.
  const status = rowStatus(workspace);
  const subHost = remote ? hostName : null;
  const totals = stat !== undefined && stat.additions + stat.deletions > 0 ? stat : null;
  const twoLines = branch !== undefined || subHost !== null || totals !== null;
  const nameParts = [workspace.displayTitle, status.label];
  if (branch !== undefined) nameParts.push(branch);
  if (subHost !== null) nameParts.push(subHost);
  if (remote && hostOffline) nameParts.push(OFFLINE_LABEL);
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
              twoLines ? " workspace-row-two" : ""
            }`}
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
              className={`workspace-row-status workspace-row-status-${status.tone}${
                status.tone === "running" ? " dot-pulse" : ""
              }`}
              aria-hidden="true"
            />
            <span className="workspace-row-body">
              <span className="workspace-row-line">
                <span className="workspace-row-title">{workspace.displayTitle}</span>
              </span>
              {twoLines ? (
                <span className="workspace-row-line workspace-row-sub">
                  {branch === undefined ? null : (
                    <span className="workspace-row-branch">{branch}</span>
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
                      <span className="workspace-row-host-name">{subHost}</span>
                    </span>
                  )}
                  {remote && hostOffline ? (
                    <span className="workspace-row-offline">{OFFLINE_LABEL}</span>
                  ) : null}
                  {totals === null ? null : (
                    <span className="workspace-row-totals">
                      <span className="workspace-row-add">
                        +{compactLineCount(totals.additions)}
                      </span>{" "}
                      <span className="workspace-row-del">
                        −{compactLineCount(totals.deletions)}
                      </span>
                    </span>
                  )}
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
      {editing ? null : (
        <button
          type="button"
          ref={kebabRef}
          className="sidebar-kebab workspace-row-kebab"
          aria-label={`Actions for ${workspace.displayTitle}`}
          aria-haspopup="menu"
          aria-expanded={menuOpen}
          onClick={() => setMenuOpen(!menuOpen)}
        >
          <span aria-hidden="true">⋯</span>
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
          {/* A remote row is renamed on its own machine: only the archive
              travels the link from here. */}
          {remote ? null : (
            <button type="button" role="menuitem" onClick={startRename}>
              Rename
            </button>
          )}
          {/* Archive takes the row off the list for every isolation; the
              folder stays, so a local row is no exception. */}
          <button type="button" role="menuitem" onClick={startArchive}>
            Archive workspace
          </button>
        </div>
      ) : null}
      {archiveRefusal !== null ? (
        // The mapped sentence only: no raw daemon text reaches this surface.
        <span className="workspace-row-delete-refusal" role="alert">
          {archiveRefusal.sentence}
        </span>
      ) : null}
      <ConfirmDialog
        open={confirming}
        title={`Archive “${workspace.displayTitle}”?`}
        message="Takes this workspace off the list. Its folder and its history stay on disk."
        confirmLabel="Archive"
        tone="accent"
        onConfirm={() => void confirmArchive()}
        onCancel={() => setConfirming(false)}
      />
    </div>
  );
});
