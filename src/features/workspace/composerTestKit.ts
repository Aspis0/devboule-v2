import { act, type ComponentProps } from "react";
import { WorkspaceComposer } from "./WorkspaceComposer";

/**
 * The fixtures and drivers the composer's two suites share — how a test types
 * into the composer, presses a key on it, and reads its menu. Nothing here is
 * a case, and nothing here is production code.
 */

export const MENU_COMMANDS = [
  { name: "build", description: "Build the app" },
  { name: "goal", description: "Set a goal" },
  { name: "workflow", description: "Run a workflow" },
];

export interface ComposerMocks {
  onSend: (text: string) => void;
  onQueue: (text: string) => void;
}

export function composerProps(
  mocks: ComposerMocks,
  overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {},
): ComponentProps<typeof WorkspaceComposer> {
  return {
    streaming: false,
    turnActive: false,
    disabled: false,
    disabledReason: null,
    availableCommands: MENU_COMMANDS,
    onSend: mocks.onSend,
    onQueue: mocks.onQueue,
    ...overrides,
  };
}

export interface ComposerDrivers {
  textarea(): HTMLTextAreaElement;
  type(text: string): Promise<void>;
  /** Dispatches one keydown and hands back the event, so a test can also read
   * whether the composer let the browser have it. */
  press(key: string, modifiers?: KeyboardEventInit): Promise<KeyboardEvent>;
  menu(): HTMLElement | null;
  menuOrFail(): HTMLElement;
  rows(): HTMLButtonElement[];
  activeRow(): HTMLElement | null;
}

export function composerDrivers(container: HTMLElement): ComposerDrivers {
  const textarea = (): HTMLTextAreaElement => {
    const element = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message the agent"]',
    );
    if (element === null) throw new Error("composer textarea did not render");
    return element;
  };

  const menu = (): HTMLElement | null =>
    container.querySelector('[aria-label="Available commands"]');

  const menuOrFail = (): HTMLElement => {
    const open = menu();
    if (open === null) throw new Error("command menu did not render");
    return open;
  };

  return {
    textarea,
    async type(text: string): Promise<void> {
      const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
      if (setValue === undefined) throw new Error("textarea value setter did not exist");
      await act(async () => {
        setValue.call(textarea(), text);
        textarea().dispatchEvent(new Event("input", { bubbles: true }));
      });
    },
    async press(key: string, modifiers: KeyboardEventInit = {}): Promise<KeyboardEvent> {
      const event = new KeyboardEvent("keydown", {
        key,
        bubbles: true,
        cancelable: true,
        ...modifiers,
      });
      await act(async () => {
        textarea().dispatchEvent(event);
      });
      return event;
    },
    menu,
    menuOrFail,
    rows: () => [...menuOrFail().querySelectorAll<HTMLButtonElement>('[role="option"]')],
    activeRow: () => {
      const id = textarea().getAttribute("aria-activedescendant");
      if (id === null) return null;
      return document.getElementById(id);
    },
  };
}
