// One browser tab: the controls above the page, and the rectangle the page
// itself is drawn in. The page is a child webview in the Rust process, not a
// DOM node, so this component owns three things instead of rendering one:
// the address bar and the history/stop controls, the rectangle measured off
// its own page area, and what happens to the page when this component goes
// away.

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { ErrorTriangleIcon } from "./ErrorTriangleIcon";
import { browserChrome, submitBrowserAddress } from "./browserChrome";
import { browserTabLabel } from "./browserUrl";
import {
  browserHistory,
  browserNavigate,
  browserOpen,
  browserPark,
  browserPresent,
  browserRectOf,
  browserReload,
} from "./browserController";
import { patchBrowserTab, requestBrowserPopup } from "./browserTabs";
import type { BrowserUpdate, BrowserViewState } from "../../types/ipc";
import { browserFocusAddress, browserReloadChord } from "../../lib/keymap";
import "./BrowserTab.css";

export interface BrowserTabProps {
  /** The controller's name for this tab's page. */
  browserId: string;
  /** Where the page starts: the tab's restored address, or the start page. */
  url: string;
}

const IDLE: BrowserViewState = {
  url: "",
  title: null,
  favicon: null,
  loading: true,
  canGoBack: false,
  canGoForward: false,
  error: null,
};

export function BrowserTab({ browserId, url }: BrowserTabProps) {
  const [page, setPage] = useState<BrowserViewState>({ ...IDLE, url });
  /** What the user has typed. Null while the bar shows the page's own
   * address, which is the only state in which a keystroke would move the
   * caret to the start on every update. */
  const [draft, setDraft] = useState<string | null>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  const addressRef = useRef<HTMLInputElement>(null);
  const pageAreaRef = useRef<HTMLDivElement>(null);
  // Read once, at open. The record's own address changes as the page moves,
  // and re-running the open effect on it would dispose the page and open it
  // again on every navigation.
  const startUrl = useRef(url).current;

  const onUpdate = useCallback(
    (update: BrowserUpdate) => {
      if (update.kind === "newWindow") {
        // The strip owns which workspace a tab belongs to, so a page asking
        // for a window is answered there rather than here.
        requestBrowserPopup(browserId, update.url);
        return;
      }
      const { kind: _kind, ...state } = update;
      setPage((current) => ({ ...current, ...state }));
      patchBrowserTab(browserId, {
        url: state.url,
        title: state.title,
        favicon: state.favicon,
      });
    },
    [browserId],
  );

  useEffect(() => {
    let disposed = false;
    void browserOpen(browserId, startUrl, onUpdate)
      .then((opened) => {
        if (!disposed) setPage(opened);
      })
      .catch((cause: unknown) => {
        if (disposed) return;
        setRefusal(typeof cause === "string" ? cause : "This page could not be opened.");
        setPage((current) => ({ ...current, loading: false }));
      });
    return () => {
      disposed = true;
      // Park, never dispose: a tab that is merely no longer in front keeps
      // its page alive and running. Disposal belongs to the tab's close, and
      // a park for a tab that is already gone has nothing to park.
      void browserPark(browserId).catch(() => undefined);
    };
  }, [browserId, onUpdate, startUrl]);

  // The rectangle, followed for as long as this tab is in front. The
  // ResizeObserver covers the pane's own resizes and the divider drag; the
  // window listener covers a window move, which changes no element's box but
  // does move every native child of the window with it.
  const present = useCallback(() => {
    const area = pageAreaRef.current;
    if (area === null) return;
    void browserPresent(browserId, browserRectOf(area.getBoundingClientRect()));
  }, [browserId]);

  useLayoutEffect(() => {
    const area = pageAreaRef.current;
    if (area === null) return;
    present();
    const observer = new ResizeObserver(present);
    observer.observe(area);
    // `capture` because a scroll does not bubble: without it an ancestor's
    // scroll would move the rectangle with nothing to announce it.
    window.addEventListener("resize", present);
    window.addEventListener("scroll", present, { capture: true, passive: true });
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", present);
      window.removeEventListener("scroll", present, { capture: true });
    };
  }, [present]);

  const chrome = useMemo(
    () =>
      browserChrome(
        {
          url: page.url,
          loading: page.loading,
          canGoBack: page.canGoBack,
          canGoForward: page.canGoForward,
          error: page.error,
        },
        draft,
      ),
    [draft, page],
  );

  const submit = useCallback(
    (event?: FormEvent) => {
      event?.preventDefault();
      const outcome = submitBrowserAddress(draft ?? chrome.barValue);
      if (outcome.url === null) {
        setRefusal(outcome.error);
        return;
      }
      setRefusal(null);
      setDraft(null);
      addressRef.current?.blur();
      void browserNavigate(browserId, outcome.url);
    },
    [browserId, chrome.barValue, draft],
  );

  const act = useCallback(
    (what: "back" | "forward" | "stop" | "reload") => {
      if (what === "reload") void browserReload(browserId);
      else void browserHistory(browserId, what);
    },
    [browserId],
  );

  // The two chords a browser tab answers. This component is mounted exactly
  // while one is in front, so the keys never reach a chat, a terminal or a
  // field behind it.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (browserFocusAddress(event)) {
        event.preventDefault();
        addressRef.current?.focus({ preventScroll: true });
        addressRef.current?.select();
        return;
      }
      if (browserReloadChord(event)) {
        event.preventDefault();
        act(page.loading ? "stop" : "reload");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [act, page.loading]);

  const errorLine = refusal ?? chrome.error;
  return (
    <div
      id="workspace-panel-terminal"
      className="workspace-conversation workspace-browser-pane"
      role="tabpanel"
      aria-label={browserTabLabel(page.title, page.url)}
    >
      <form className="browser-chrome" onSubmit={submit}>
        <button
          type="button"
          className="browser-chrome-button"
          aria-label="Back"
          title="Back"
          disabled={!chrome.canGoBack}
          onClick={() => act("back")}
        >
          <BrowserGlyph d="M9 3 4 7l5 4M4 7h7" />
        </button>
        <button
          type="button"
          className="browser-chrome-button"
          aria-label="Forward"
          title="Forward"
          disabled={!chrome.canGoForward}
          onClick={() => act("forward")}
        >
          <BrowserGlyph d="M5 3l5 4-5 4M10 7H3" />
        </button>
        <button
          type="button"
          className="browser-chrome-button"
          aria-label={chrome.action === "stop" ? "Stop loading this page" : "Reload this page"}
          title={chrome.action === "stop" ? "Stop" : "Reload"}
          onClick={() => act(chrome.action === "stop" ? "stop" : "reload")}
        >
          {chrome.action === "stop" ? (
            <BrowserGlyph d="M5 5h4v4H5z" />
          ) : (
            <BrowserGlyph d="M11 7a4 4 0 1 1-1.2-2.8M11 3v2.5H8.5" />
          )}
        </button>
        <input
          ref={addressRef}
          className="browser-address"
          type="text"
          value={chrome.barValue}
          spellCheck={false}
          autoComplete="off"
          aria-label="Address"
          aria-invalid={errorLine !== null}
          onFocus={(event) => event.currentTarget.select()}
          onChange={(event) => {
            setDraft(event.currentTarget.value);
            setRefusal(null);
          }}
          onKeyDown={(event: ReactKeyboardEvent<HTMLInputElement>) => {
            if (event.key === "Escape") {
              setDraft(null);
              setRefusal(null);
            }
          }}
        />
      </form>
      {errorLine !== null ? (
        <div className="browser-error" role="alert">
          <ErrorTriangleIcon />
          <span className="browser-error-text">{errorLine}</span>
        </div>
      ) : null}
      {/* The page is drawn over this box by a child webview in Rust, so the
          box holds no page of its own: it is the rectangle, kept visible so
          that a child which fails to be placed reads as a blank pane instead
          of an invisible one. */}
      <div
        ref={pageAreaRef}
        className={`browser-page-area${page.loading ? " browser-page-area-loading" : ""}`}
        data-browser-id={browserId}
        aria-busy={page.loading}
      />
    </div>
  );
}

/** 14 px chrome glyph, drawn the way the strip draws its kind marks. */
function BrowserGlyph({ d }: { d: string }) {
  return (
    <svg width={14} height={14} viewBox="0 0 14 14" aria-hidden="true" focusable="false">
      <path
        d={d}
        fill="none"
        stroke="currentColor"
        strokeWidth={1.4}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
