// One pencil + caret action shared by the File and Diff tab headers: the
// pencil opens the preferred editor target with the tab's file (the diff
// adds its first hunk's line), the caret picks which target that is, and
// both answer only inside this header — the refusal line, the launch
// failure, the honest disabled state. The tab supplies its workspace id and
// relative path; the command resolves the root and the canonical file, so
// no host path ever crosses to this side.

import {
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import { ErrorText } from "../../components/ErrorText";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { useMenuOpen } from "../../lib/menuOpen";
import { editorTargetsList, workspaceFileOpen, type EditorTarget } from "../../lib/tauri";
import {
  PREFERRED_EDITOR_STORAGE_KEY,
  readPreferredEditorId,
  resolvePreferredEditorId,
  writePreferredEditorId,
} from "./preferredEditor";
import { AnchoredPopover } from "./popoverPlace";
import { moveMenuFocus } from "./strip/menuNav";
import "./OpenInEditorAction.css";

const REMOTE_REFUSAL =
  "This workspace is on another device; its files cannot be opened by an editor on this computer.";
const NO_TARGET_TOOLTIP = "No editor found on this computer";

function PencilGlyph(): ReactNode {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path
        d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.75"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function without(keys: ReadonlySet<string>, key: string): ReadonlySet<string> {
  if (!keys.has(key)) return keys;
  const next = new Set(keys);
  next.delete(key);
  return next;
}

export function OpenInEditorAction({
  workspaceId,
  path,
  line,
  servedRemotely = false,
}: {
  workspaceId: string;
  path: string;
  /** The diff's first hunk's new-side line; a file tab passes none. */
  line?: number;
  /**
   * Another device serves this folder: refused before any request, since this
   * machine cannot open a folder it does not hold. No roster field carries a
   * host yet, so nothing passes this — the refusal is the guard, not the default.
   */
  servedRemotely?: boolean;
}) {
  const failureId = useId();
  const targetKey = `${workspaceId}\u0000${path}`;
  const [targets, setTargets] = useState<EditorTarget[] | null>(null);
  const [saved, setSaved] = useState<string | null>(() => readPreferredEditorId());
  const [notice, setNotice] = useState<{ key: string; value: ErrorSentence } | null>(null);
  const [pendingKeys, setPendingKeys] = useState<ReadonlySet<string>>(() => new Set());
  const [menuOpen, setMenuOpen] = useState(false);
  const caretRef = useRef<HTMLButtonElement | null>(null);
  const menuRootRef = useRef<HTMLDivElement | null>(null);
  const menuListRef = useRef<HTMLDivElement | null>(null);

  // A result only speaks for the key it was issued under: changing the
  // file or the workspace clears this header's line at once, and a slow
  // result for another key can neither land here nor erase what is shown.
  const [seenKey, setSeenKey] = useState(targetKey);
  if (seenKey !== targetKey) {
    setSeenKey(targetKey);
    setNotice(null);
  }

  useEffect(() => {
    let live = true;
    editorTargetsList().then(
      (found) => {
        if (live) setTargets(found);
      },
      () => {
        if (live) setTargets([]);
      },
    );
    return () => {
      live = false;
    };
  }, []);

  // The preference is one store for every window: `storage` fires only for
  // another document's write, which is exactly the cross-window edge this
  // state would otherwise miss until a remount.
  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key !== null && event.key !== PREFERRED_EDITOR_STORAGE_KEY) return;
      setSaved(readPreferredEditorId());
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  useMenuOpen(menuOpen, () => setMenuOpen(false));

  useEffect(() => {
    if (!menuOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return;
      if (menuRootRef.current?.contains(event.target)) return;
      if (caretRef.current?.contains(event.target)) return;
      setMenuOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [menuOpen]);

  useEffect(() => {
    if (!menuOpen) return;
    menuListRef.current
      ?.querySelector<HTMLButtonElement>("[role='menuitemradio']")
      ?.focus({ preventScroll: true });
  }, [menuOpen]);

  const available = targets ?? [];
  const preferredId = resolvePreferredEditorId(
    saved,
    available.map((target) => target.id),
  );
  // A saved id this machine lost normalizes back to the fallback, so storage
  // stops carrying a name no target answers to. The store is re-read first:
  // an id another window just wrote is not the stale one.
  useEffect(() => {
    if (saved === null || preferredId === null || preferredId === saved) return;
    if (readPreferredEditorId() !== saved) return;
    writePreferredEditorId(preferredId);
  }, [saved, preferredId]);
  const failure = notice !== null && notice.key === targetKey ? notice.value : null;
  const pending = pendingKeys.has(targetKey);
  const noTarget = targets !== null && preferredId === null;
  const disabled = !servedRemotely && (targets === null || preferredId === null || pending);

  const open = (): void => {
    if (servedRemotely) {
      setNotice({ key: targetKey, value: { sentence: REMOTE_REFUSAL, detail: null } });
      return;
    }
    if (disabled || preferredId === null) return;
    const issuedKey = targetKey;
    setNotice((current) => (current === null || current.key === issuedKey ? null : current));
    setPendingKeys((keys) => new Set(keys).add(issuedKey));
    workspaceFileOpen(workspaceId, path, line, preferredId).then(
      () => {
        setPendingKeys((keys) => without(keys, issuedKey));
        setNotice((current) => (current !== null && current.key === issuedKey ? null : current));
      },
      (cause: unknown) => {
        setPendingKeys((keys) => without(keys, issuedKey));
        const value = errorSentence(cause);
        setNotice((current) =>
          current === null || current.key === issuedKey ? { key: issuedKey, value } : current,
        );
      },
    );
  };

  const closeMenu = (): void => {
    if (menuRootRef.current?.contains(document.activeElement)) {
      caretRef.current?.focus({ preventScroll: true });
    }
    setMenuOpen(false);
  };

  const onMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>): void => {
    if (event.key === "Escape" || event.key === "Tab") {
      event.preventDefault();
      closeMenu();
      return;
    }
    moveMenuFocus(menuListRef.current, event);
  };

  return (
    <span className="open-in-editor">
      {failure !== null ? (
        <span className="open-in-editor-failure" role="alert">
          <ErrorText sentence={failure.sentence} detail={failure.detail} id={failureId} />
        </span>
      ) : null}
      <button
        type="button"
        className="open-in-editor-button"
        aria-label="Open in editor"
        title={noTarget ? NO_TARGET_TOOLTIP : "Open in editor"}
        disabled={disabled}
        onClick={open}
      >
        <PencilGlyph />
      </button>
      <button
        ref={caretRef}
        type="button"
        className="open-in-editor-caret"
        aria-label="Choose editor"
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        title={noTarget ? NO_TARGET_TOOLTIP : undefined}
        disabled={targets === null || available.length === 0}
        onClick={() => setMenuOpen((value) => !value)}
      >
        ▾
      </button>
      {menuOpen ? (
        <AnchoredPopover
          anchorRef={caretRef}
          containerRef={menuRootRef}
          onDismiss={() => setMenuOpen(false)}
          className="open-in-editor-menu"
        >
          <div ref={menuListRef} role="menu" aria-label="Open with" onKeyDown={onMenuKeyDown}>
            {available.map((target) => (
              <button
                key={target.id}
                type="button"
                role="menuitemradio"
                aria-checked={target.id === preferredId}
                className="open-in-editor-menu-item"
                onClick={() => {
                  writePreferredEditorId(target.id);
                  setSaved(target.id);
                  closeMenu();
                }}
              >
                {target.label}
              </button>
            ))}
          </div>
        </AnchoredPopover>
      ) : null}
    </span>
  );
}
