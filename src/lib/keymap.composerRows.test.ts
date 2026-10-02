import { describe, expect, it } from "vitest";
import {
  COMMAND_MENU_KEY,
  composerChordLabel,
  shortcutSections,
  type ComposerKeyAction,
} from "./keymap";
import type { SendBehavior } from "./sendBehavior";

/** The copy each composer state is expected to produce, written by hand; it
 * does not run WorkspaceComposer's branches. */
type ComposerAct = "queues" | "sends" | "nothing";

const BLOCKED = { blocked: true, submit: "nothing", alternate: "nothing" } as const;

const COMPOSER_BRANCHES: readonly {
  behavior: SendBehavior;
  queueAvailable: boolean;
  blocked: boolean;
  submit: ComposerAct;
  alternate: ComposerAct;
}[] = [
  { behavior: "queue", queueAvailable: true, blocked: false, submit: "queues", alternate: "sends" },
  { behavior: "queue", queueAvailable: false, blocked: false, submit: "sends", alternate: "sends" },
  {
    behavior: "interrupt-and-send",
    queueAvailable: true,
    blocked: false,
    submit: "sends",
    alternate: "queues",
  },
  {
    behavior: "interrupt-and-send",
    queueAvailable: false,
    blocked: false,
    submit: "sends",
    alternate: "nothing",
  },
  { behavior: "queue", queueAvailable: true, ...BLOCKED },
  { behavior: "queue", queueAvailable: false, ...BLOCKED },
  { behavior: "interrupt-and-send", queueAvailable: true, ...BLOCKED },
  { behavior: "interrupt-and-send", queueAvailable: false, ...BLOCKED },
];

function composerSection(behavior: SendBehavior) {
  const found = shortcutSections(behavior).find((section) => section.label === "Composer");
  if (found === undefined) throw new Error("no Composer section");
  return found;
}

function composerRow(behavior: SendBehavior, action: ComposerKeyAction): string {
  const found = composerSection(behavior).rows.find(
    (row) => row.keys === composerChordLabel(action),
  );
  if (found === undefined) throw new Error(`no composer row for ${action}`);
  return `${found.title} — ${found.detail ?? ""}`;
}

function readyActs(behavior: SendBehavior, key: "submit" | "alternate"): readonly ComposerAct[] {
  return COMPOSER_BRANCHES.filter((branch) => !branch.blocked && branch.behavior === behavior).map(
    (branch) => branch[key],
  );
}

describe("the composer rows state the rule the composer runs", () => {
  it("swaps what Enter and the modifier do with the send setting", () => {
    expect(composerRow("queue", "submit")).toContain("Enter queues the message");
    expect(composerRow("queue", "alternate")).toContain("Interrupt and send");
    expect(composerRow("queue", "submit")).not.toContain("Enter interrupts");
    expect(composerRow("interrupt-and-send", "submit")).toContain(
      "Enter interrupts the turn and sends the message",
    );
    expect(composerRow("interrupt-and-send", "alternate")).toContain("Queue the message");
    expect(composerRow("interrupt-and-send", "submit")).not.toContain("Enter queues");
  });

  it("names the queue condition on the row availability decides", () => {
    for (const behavior of ["queue", "interrupt-and-send"] as const) {
      for (const key of ["submit", "alternate"] as const) {
        const copy = composerRow(behavior, key);
        if (new Set(readyActs(behavior, key)).size > 1) {
          expect(copy).toContain("when queueing is available");
        } else {
          expect(copy).not.toContain("when queueing is available");
        }
      }
    }
  });

  it("says what each ready branch does, and nothing it does not", () => {
    for (const branch of COMPOSER_BRANCHES.filter((candidate) => !candidate.blocked)) {
      for (const key of ["submit", "alternate"] as const) {
        const copy = composerRow(branch.behavior, key);
        const act = branch[key];
        if (act === "queues") expect(copy).toContain("queues the message");
        if (act === "sends") expect(copy).toContain("sends");
        if (act === "nothing") {
          expect(copy).not.toContain("sends");
          expect(copy).toContain("the draft stays in the composer");
        }
      }
    }
  });

  it("says once, on the group, that the blocked states are no-ops", () => {
    for (const branch of COMPOSER_BRANCHES.filter((candidate) => candidate.blocked)) {
      expect([branch.submit, branch.alternate]).toEqual(["nothing", "nothing"]);
    }
    const note = composerSection("queue").note;
    expect(note).toBe(
      "Nothing is sent or queued while the composer is disabled or an image send is in progress.",
    );
    expect(composerSection("interrupt-and-send").note).toBe(note);
  });

  it("says a visible command suggestion takes Enter before the send", () => {
    for (const behavior of ["queue", "interrupt-and-send"] as const) {
      const row = composerSection(behavior).rows.find(
        (candidate) => candidate.keys === COMMAND_MENU_KEY,
      );
      expect(row?.detail).toContain(
        "While they show, Enter picks the highlighted command instead of sending.",
      );
    }
  });
});
