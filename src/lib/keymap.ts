/**
 * The app's shortcut keymap: the matchers the handlers run and the rows the
 * Shortcuts page lists. Both read the same definitions, so every listed chord
 * is a chord the app answers. Keys a widget owns only while it is open —
 * command-menu travel, dialog Escape — stay local to that widget and are not
 * listed here as app shortcuts.
 */
import { isImeComposition } from "./imeComposition";
import type { SendBehavior } from "./sendBehavior";

/** What a bare arrow or Home/End asks a focused tab list for. The strip and
 * the side-panel tab row read the same matcher. */
export type TabKeyMove = "next" | "previous" | "first" | "last";

const TAB_MOVES: readonly (readonly [string, TabKeyMove])[] = [
  ["ArrowRight", "next"],
  ["ArrowDown", "next"],
  ["ArrowLeft", "previous"],
  ["ArrowUp", "previous"],
  ["Home", "first"],
  ["End", "last"],
];

const TAB_MOVES_BY_KEY = new Map<string, TabKeyMove>(TAB_MOVES);

export function tabMoveForKey(key: string): TabKeyMove | null {
  return TAB_MOVES_BY_KEY.get(key) ?? null;
}

const CLOSE_TAB_KEYS: readonly string[] = ["Delete", "Backspace"];

/** Delete and Backspace close the focused strip tab; panel tabs have no
 * close key. */
export function isCloseTabKey(key: string): boolean {
  return CLOSE_TAB_KEYS.some((candidate) => candidate === key);
}

/** Which strip tab the window-level chord walks to. */
export type StripChord = "next" | "previous";

const STRIP_CHORDS: readonly { chord: StripChord; key: string }[] = [
  { chord: "next", key: "]" },
  { chord: "previous", key: "[" },
];

/** True when the event belongs to a widget that owns the whole keyboard: a
 * terminal, or an open menu, dialog or listbox. The chord must not pull its
 * own keys out from under one. */
function holdsTheKeyboard(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target.closest(".workspace-terminal-shell") !== null ||
    target.closest('[role="menu"], [role="dialog"], [role="alertdialog"], [role="listbox"]') !==
      null
  );
}

/** True when the event belongs to a widget that owns its keys: a text field,
 * a terminal, or an open menu, dialog or listbox. The chord must not pull a
 * caret or a terminal's own Alt chord out from under it. */
function ownsItsKeys(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    target.isContentEditable ||
    holdsTheKeyboard(target)
  );
}

/**
 * The window-level Alt+Shift+[ / Alt+Shift+] chord, or null. Composition and
 * target are part of the match: the chord is text inside a field, and a
 * terminal or an open menu keeps its own Alt keys.
 */
export function stripChordFor(
  event: Pick<
    KeyboardEvent,
    "key" | "altKey" | "shiftKey" | "ctrlKey" | "metaKey" | "isComposing" | "keyCode"
  > & { target: EventTarget | null },
): StripChord | null {
  if (!event.altKey || !event.shiftKey || event.ctrlKey || event.metaKey) return null;
  const found = STRIP_CHORDS.find((entry) => entry.key === event.key);
  if (found === undefined) return null;
  if (isImeComposition(event)) return null;
  if (ownsItsKeys(event.target)) return null;
  return found.chord;
}

/** Enter's three answers in the composer. */
export type ComposerKeyAction = "submit" | "alternate" | "newline";

/**
 * Enter submits, Shift+Enter makes a newline, and the command modifier asks
 * for the other action. Either Ctrl or Meta counts on every OS — the display
 * prints the host's own key — and Alt rides with the plain key.
 */
export function composerKeyAction(
  event: Pick<
    KeyboardEvent,
    "key" | "shiftKey" | "ctrlKey" | "metaKey" | "isComposing" | "keyCode"
  >,
): ComposerKeyAction | null {
  if (isImeComposition(event)) return null;
  if (event.key !== "Enter") return null;
  if (event.shiftKey) return "newline";
  if (event.ctrlKey || event.metaKey) return "alternate";
  return "submit";
}

/** The keys as the app prints them, per composer action. */
export function composerChordLabel(action: ComposerKeyAction): string {
  switch (action) {
    case "submit":
      return "Enter";
    case "alternate":
      return `${commandModifier()}+Enter`;
    case "newline":
      return "Shift+Enter";
  }
}

/** Which way the crescent band pages. */
export type CrescentPage = "next" | "previous";

const CRESCENT_PAGE_KEYS: readonly { page: CrescentPage; key: string }[] = [
  { page: "previous", key: "ArrowLeft" },
  { page: "next", key: "ArrowRight" },
];

/** The band's paging keys. The caller owns the open/closed state and the
 * layout's canPrev/canNext. */
export function crescentPageForKey(key: string): CrescentPage | null {
  const found = CRESCENT_PAGE_KEYS.find((entry) => entry.key === key);
  return found?.page ?? null;
}

/** Typing this at the start of a message opens the command menu. */
export const COMMAND_MENU_KEY = "/";

/** The key the search chord rides on, with either command modifier. */
const SEARCH_KEY = "k";

/**
 * The window-level chord that focuses the sidebar's search field, or false.
 * A text field does not claim it: the whole point is reaching the search
 * from the composer, so only the surfaces that take the whole keyboard — a
 * terminal, menu, dialog or listbox — hold it back.
 */
export function searchChordFor(
  event: Pick<
    KeyboardEvent,
    "key" | "altKey" | "ctrlKey" | "metaKey" | "isComposing" | "keyCode"
  > & { target: EventTarget | null },
): boolean {
  if (isImeComposition(event)) return false;
  if (event.altKey) return false;
  // Exactly one command modifier: Cmd on a Mac, Ctrl everywhere else.
  if (event.ctrlKey === event.metaKey) return false;
  if (event.key.toLowerCase() !== SEARCH_KEY) return false;
  return !holdsTheKeyboard(event.target);
}

/** Whether the event is a command-modified plain letter, which is the shape
 * both browser chords share. Either modifier counts, as everywhere else in
 * this keymap; Alt and Shift make it a different chord. */
function isCommandLetter(
  event: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey">,
): boolean {
  return (event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey;
}

/** The two chords as a keymap matcher is given one: a chord is only a chord
 * outside a field, a terminal or an open dialog, and never mid-composition. */
type BrowserChordEvent = Pick<
  KeyboardEvent,
  "key" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey" | "isComposing" | "keyCode"
> & { target: EventTarget | null };

/** The same two guards every other matcher in this file applies. A browser
 * tab's chords are window-level, so without them Ctrl+L inside a dialog would
 * pull the focus out of the dialog and into a pane behind it. */
function browserChordAllowed(event: BrowserChordEvent): boolean {
  if (isImeComposition(event)) return false;
  if (ownsItsKeys(event.target)) return false;
  return true;
}

/**
 * The two chords a browser tab answers, and only while one is in front: the
 * command modifier with L focuses the address bar and selects what is in it,
 * and with R reloads — or stops a load in progress, which is what the tab's
 * own reload button does at that moment.
 *
 * Read here rather than in the pane so the Shortcuts page and the keymap
 * cannot disagree; the pane binds them while it is mounted, which is exactly
 * while a browser tab is the active tab.
 */
export function browserFocusAddress(event: BrowserChordEvent): boolean {
  return (
    isCommandLetter(event) && event.key.toLowerCase() === "l" && browserChordAllowed(event)
  );
}

export function browserReloadChord(event: BrowserChordEvent): boolean {
  return (
    isCommandLetter(event) && event.key.toLowerCase() === "r" && browserChordAllowed(event)
  );
}

/** The command modifier's label for a platform string. */
export function commandModifierLabel(platform: string): "Cmd" | "Ctrl" {
  return platform.startsWith("Mac") ? "Cmd" : "Ctrl";
}

/** The key this host draws for the command modifier, read per call: a
 * module-scope constant would answer for a test's platform before it runs. */
export function commandModifier(): "Cmd" | "Ctrl" {
  return commandModifierLabel(typeof navigator === "undefined" ? "" : navigator.platform);
}

/** The search chord as the app prints it, on this host's modifier. */
export function searchChordLabel(): string {
  return `${commandModifier()}+${SEARCH_KEY.toUpperCase()}`;
}

/** One line of the Shortcuts page. */
interface ShortcutRow {
  /** The keys, as the page prints them. */
  keys: string;
  /** What the keys do. */
  title: string;
  /** The condition the keys alone do not carry. */
  detail?: string;
}

interface ShortcutSection {
  label: string;
  /** One line for the whole group, when a condition holds for every row. */
  note?: string;
  rows: readonly ShortcutRow[];
}

function tabMoveKeys(move: TabKeyMove): string {
  return TAB_MOVES.filter(([, candidate]) => candidate === move)
    .map(([key]) => key)
    .join(" / ");
}

function tabStripRows(): readonly ShortcutRow[] {
  const chordRows = STRIP_CHORDS.map(({ chord, key }) => ({
    keys: `Alt+Shift+${key}`,
    title: chord === "next" ? "Next tab" : "Previous tab",
    detail: "Not while typing in a field or terminal, or while a menu, list or dialog is open.",
  }));
  return [
    ...chordRows,
    { keys: tabMoveKeys("next"), title: "Next tab", detail: "With the tab list focused." },
    { keys: tabMoveKeys("previous"), title: "Previous tab", detail: "With the tab list focused." },
    {
      keys: `${tabMoveKeys("first")} / ${tabMoveKeys("last")}`,
      title: "First or last tab",
      detail: "With the tab list focused.",
    },
    { keys: CLOSE_TAB_KEYS.join(" / "), title: "Close the focused tab" },
  ];
}

function composerRows(behavior: SendBehavior): readonly ShortcutRow[] {
  const queued = behavior === "queue";
  return [
    {
      keys: composerChordLabel("submit"),
      title: "Send the message",
      detail: queued
        ? "While the agent is working, Enter queues the message when queueing is available; otherwise it sends."
        : "While the agent is working, Enter interrupts the turn and sends the message.",
    },
    { keys: composerChordLabel("newline"), title: "Start a new line" },
    {
      keys: composerChordLabel("alternate"),
      title: queued ? "Interrupt and send" : "Queue the message",
      detail: queued
        ? "While the agent is working, this interrupts the turn and sends the message."
        : "While the agent is working, this queues the message when queueing is available; otherwise the draft stays in the composer.",
    },
    {
      keys: COMMAND_MENU_KEY,
      title: "Open the command menu",
      detail:
        "Type it at the start of a message to see the commands this agent offers. While they show, Enter picks the highlighted command instead of sending.",
    },
  ];
}

function crescentRows(): readonly ShortcutRow[] {
  return CRESCENT_PAGE_KEYS.map(({ page, key }) => ({
    keys: key,
    title: page === "next" ? "Next surface" : "Previous surface",
    detail: "While the surface list is open, outside text fields and dialogs.",
  }));
}

function navigationRows(): readonly ShortcutRow[] {
  return [
    {
      keys: searchChordLabel(),
      title: "Search workspaces",
      detail: "The workspaces panel opens first while it is collapsed.",
    },
    ...crescentRows(),
  ];
}

function panelRows(): readonly ShortcutRow[] {
  return [
    { keys: tabMoveKeys("next"), title: "Next panel tab" },
    { keys: tabMoveKeys("previous"), title: "Previous panel tab" },
    {
      keys: `${tabMoveKeys("first")} / ${tabMoveKeys("last")}`,
      title: "First or last panel tab",
    },
  ];
}

function browserRows(): readonly ShortcutRow[] {
  return [
    {
      keys: `${commandModifier()}+L`,
      title: "Focus the address bar",
      detail: "While a browser tab is in front.",
    },
    {
      keys: `${commandModifier()}+R`,
      title: "Reload the page, or stop it while it loads",
      detail: "While a browser tab is in front.",
    },
  ];
}

/** Both composer keys are no-ops while the composer is disabled or an image
 * send is in flight — sendInput and queueInput return early on both — so the
 * page says it once, on the group. */
const COMPOSER_BLOCKED_NOTE =
  "Nothing is sent or queued while the composer is disabled or an image send is in progress.";

/** The Shortcuts page's rows, grouped by where the keys work. `behavior` is
 * the Editing page's Default send: the two Enter rows say what Enter and the
 * command modifier do under the current choice. */
export function shortcutSections(behavior: SendBehavior): readonly ShortcutSection[] {
  return [
    { label: "Tabs", rows: tabStripRows() },
    { label: "Composer", rows: composerRows(behavior), note: COMPOSER_BLOCKED_NOTE },
    { label: "Navigation", rows: navigationRows() },
    { label: "Panel", rows: panelRows() },
    { label: "Browser", rows: browserRows() },
  ];
}
