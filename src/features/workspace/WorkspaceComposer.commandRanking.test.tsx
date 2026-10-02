// The slash menu's row ORDER under a query: the exact match rises above the
// provider's rows that merely start with (or contain) the query, Enter takes
// that first row, and the match set itself stays the substring set. The
// fixture reproduces the real pi case: the provider publishes goal-* commands
// and the surface appends the universal /goal after them
// (`AgentChatSurface.withGoalCommand`), so source order puts /goal last.
// The menu's keys, dismissal and states are
// `WorkspaceComposer.commandMenu.test.tsx`.
// @vitest-environment happy-dom
import { act, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { composerDrivers, type ComposerDrivers, type ComposerMocks } from "./composerTestKit";
import type { PromptAttachment } from "../../types/ipc";
import { WorkspaceComposer } from "./WorkspaceComposer";
import type { WorkspaceCommand } from "./WorkspaceCommandMenu";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const GOAL_PROVIDER_COMMANDS: WorkspaceCommand[] = [
  "goal-archive",
  "goal-board",
  "goal-check",
  "goal-detail",
  "goal-edit",
  "goal-focus",
  "goal-guide",
  "goal-history",
  "goal-inspect",
  "goal-jump",
  "goal-list",
  "goal-move",
  "goal-new",
  "goal-open",
].map((name) => ({ name, description: `Provider command ${name}` }));

const WITH_APPENDED_GOAL: WorkspaceCommand[] = [
  ...GOAL_PROVIDER_COMMANDS,
  { name: "goal", description: "Set a goal" },
];

let container: HTMLDivElement;
let root: Root | null = null;
let onSend: Mock<(text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>>;
let onQueue: Mock<(text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>>;
let mocks: ComposerMocks;
let drive: ComposerDrivers;

async function renderComposer(
  overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {},
): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root!.render(
      <WorkspaceComposer
        streaming={false}
        turnActive={false}
        disabled={false}
        disabledReason={null}
        availableCommands={WITH_APPENDED_GOAL}
        onSend={mocks.onSend}
        onQueue={mocks.onQueue}
        {...overrides}
      />,
    );
  });
}

function firstName(row: HTMLButtonElement): string | undefined {
  return row.querySelector(".workspace-command-name")?.textContent;
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  onSend = vi.fn<(text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>>();
  onQueue = vi.fn<(text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>>();
  mocks = { onSend, onQueue };
  drive = composerDrivers(container);
});

afterEach(async () => {
  if (root !== null) await act(async () => root!.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("the composer's ranked slash matches", () => {
  it("lists /goal first under /go, ahead of the provider's goal-* rows", async () => {
    await renderComposer();
    await drive.type("/go");

    expect(drive.rows()).toHaveLength(GOAL_PROVIDER_COMMANDS.length + 1);
    expect(firstName(drive.rows()[0])).toBe("/goal");
  });

  it("puts the typed /goal query's exact match first", async () => {
    await renderComposer();
    await drive.type("/goal");

    expect(firstName(drive.rows()[0])).toBe("/goal");
  });

  it("takes the first row on Enter, which is /goal", async () => {
    await renderComposer();
    await drive.type("/go");
    await drive.press("Enter");

    expect(onSend).not.toHaveBeenCalled();
    expect(drive.textarea().value).toBe("/goal ");
    expect(drive.menu()).toBeNull();
  });
});
