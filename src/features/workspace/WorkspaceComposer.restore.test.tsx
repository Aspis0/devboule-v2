// @vitest-environment happy-dom

// The queue hand-back applied to the composer: a refused send (or an Edit)
// must land without eating what the user typed while it was in flight —
// refused text first, the newer text after it — must never rewrite the value
// out from under an open IME composition, and must apply once per nonce.
import { act, useEffect, useState, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import {
  composerDrivers,
  composerProps,
  type ComposerDrivers,
  type ComposerMocks,
} from "./composerTestKit";
import { WorkspaceComposer } from "./WorkspaceComposer";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;
let onSend: Mock<(text: string) => void>;
let onQueue: Mock<(text: string) => void>;
let mocks: ComposerMocks;
let drive: ComposerDrivers;

function renderComposer(overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {}): void {
  root.render(<WorkspaceComposer {...composerProps(mocks, overrides)} />);
}

/** The race window itself: a keystroke that lands after the hand-back's
 * effect has run but before its update commits. Its value then replaces the
 * queued merge. `cleared` empties the field — a genuine change against the
 * committed value, so it reaches onChange; the same write on an empty
 * composer never can: React restores the node to the committed state after
 * every input event, and an input event equal to that state is deduped
 * (measured — x then "" fires once, x then y fires twice). */
function RaceKeystroke({ nonce, cleared = false }: { nonce: number; cleared?: boolean }): null {
  useEffect(() => {
    const textarea = document.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message the agent"]',
    );
    if (textarea === null) return;
    const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
    if (setValue === undefined) return;
    setValue.call(textarea, cleared ? "" : `${textarea.value}!`);
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
  }, [nonce, cleared]);
  return null;
}

/** Two refusals with one racing keystroke between them. The wrapper is
 * present from the first render so the composer instance — and its draft —
 * survive; `armed` starts the chain. Children's effects (the composer's
 * hand-back, then the racer) run before this parent's, so flipping `armed`
 * delivers the first refusal, the keystroke and the queued second refusal
 * across consecutive flushes of the same act. */
function TwoHandBacks({
  first,
  second,
  armed,
}: {
  first: { text: string; focus: boolean; nonce: number };
  second: { text: string; focus: boolean; nonce: number };
  armed: boolean;
}) {
  const [restore, setRestore] = useState<{ text: string; focus: boolean; nonce: number } | null>(
    null,
  );
  useEffect(() => {
    if (!armed) return;
    if (restore === null) {
      setRestore(first);
      return;
    }
    if (restore === first) setRestore(second);
  }, [armed, restore, first, second]);
  return (
    <>
      <WorkspaceComposer {...composerProps(mocks, { restoreDraft: restore })} />
      {restore === null ? null : <RaceKeystroke nonce={first.nonce} />}
    </>
  );
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  onSend = vi.fn<(text: string) => void>();
  onQueue = vi.fn<(text: string) => void>();
  mocks = { onSend, onQueue };
  drive = composerDrivers(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("a refused draft handed back to the composer", () => {
  it("restores into an empty composer, and stacks a later refusal above text typed since", async () => {
    await act(async () => {
      renderComposer({
        restoreDraft: { text: "refused one", focus: true, nonce: 1 },
      });
    });
    expect(drive.textarea().value).toBe("refused one");

    await drive.type("typed while the first send was in flight");

    // A second refusal lands late: the typed text must survive under it.
    await act(async () => {
      renderComposer({
        restoreDraft: { text: "refused two", focus: true, nonce: 2 },
      });
    });

    const expected = `refused two\n\ntyped while the first send was in flight`;
    expect(drive.textarea().value).toBe(expected);
  });

  it("applies a hand-back once per nonce, even when the caller re-renders with a fresh object", async () => {
    await act(async () => {
      renderComposer({ restoreDraft: { text: "refused", focus: true, nonce: 1 } });
    });
    expect(drive.textarea().value).toBe("refused");

    // Same nonce, new object identity: a caller re-rendering must not
    // prepend the refused text a second time.
    await act(async () => {
      renderComposer({ restoreDraft: { text: "refused", focus: true, nonce: 1 } });
    });
    expect(drive.textarea().value).toBe("refused");
  });

  it("parks a hand-back that lands mid-composition and applies it on compositionend", async () => {
    await act(async () => {
      renderComposer();
    });
    await drive.type("draft ");

    // The user is mid-composition: the IME owns the field right now, and
    // the composing text keeps arriving through input events.
    await act(async () => {
      drive.textarea().dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    });
    await drive.type("draft text");

    // A refusal lands while the composition is open: applying it now would
    // write the value and cancel the composition, dropping the preedit.
    await act(async () => {
      renderComposer({ restoreDraft: { text: "refused", focus: true, nonce: 3 } });
    });
    expect(drive.textarea().value).toBe("draft text");

    // The composition ends: the parked hand-back applies, and the text
    // composed meanwhile survives under it.
    await act(async () => {
      drive.textarea().dispatchEvent(new CompositionEvent("compositionend", { bubbles: true }));
    });
    expect(drive.textarea().value).toBe("refused\n\ndraft text");
  });

  it("keeps both when a keystroke races the hand-back's commit", async () => {
    await act(async () => {
      renderComposer();
    });
    await drive.type("typed text");

    // The restore and the racing keystroke land in the same commit; the
    // racer's effect runs after the hand-back's effect, so its value would
    // replace the queued merge.
    await act(async () => {
      root.render(
        <>
          <WorkspaceComposer
            {...composerProps(mocks, { restoreDraft: { text: "refused", focus: false, nonce: 5 } })}
          />
          <RaceKeystroke nonce={5} />
        </>,
      );
    });

    expect(drive.textarea().value).toBe("refused\n\ntyped text!");
  });

  it("keeps a refusal a racing clear tried to replace, and never resurrects a deliberate delete", async () => {
    await act(async () => {
      renderComposer();
    });
    await drive.type("draft");

    // The refusal's merge races a keystroke that empties the field — the
    // reviewer's P2 interleaving. (The same race on an *empty* composer is
    // unmaterializable: an input event equal to the committed value never
    // reaches onChange — see RaceKeystroke.) The refusal must survive it.
    await act(async () => {
      root.render(
        <>
          <WorkspaceComposer
            {...composerProps(mocks, { restoreDraft: { text: "refused", focus: false, nonce: 7 } })}
          />
          <RaceKeystroke nonce={7} cleared />
        </>,
      );
    });
    expect(drive.textarea().value).toBe("refused");

    // After the merge is committed the user deliberately clears — the
    // refusal must never come back, and the text they write after it stands
    // alone.
    await drive.type("");
    expect(drive.textarea().value).toBe("");
    await drive.type("hi");
    expect(drive.textarea().value).toBe("hi");
  });

  it("keeps both refusals that race one keystroke, in order", async () => {
    await act(async () => {
      root.render(
        <TwoHandBacks
          first={{ text: "refused one", focus: false, nonce: 1 }}
          second={{ text: "refused two", focus: false, nonce: 2 }}
          armed={false}
        />,
      );
    });
    await drive.type("typed text");

    // One act, the reviewer's interleaving: the first refusal hands back,
    // the racer's keystroke lands in the same window, and the parent queues
    // the second refusal right after — children's effects run before the
    // parent's, all inside this act's flush loop.
    await act(async () => {
      root.render(
        <TwoHandBacks
          first={{ text: "refused one", focus: false, nonce: 1 }}
          second={{ text: "refused two", focus: false, nonce: 2 }}
          armed
        />,
      );
    });

    expect(drive.textarea().value).toBe("refused two\n\nrefused one\n\ntyped text!");
  });

  it("never resurrects a refusal the user deleted after the merge committed", async () => {
    await act(async () => {
      renderComposer();
    });
    await act(async () => {
      renderComposer({ restoreDraft: { text: "refused", focus: false, nonce: 9 } });
    });
    expect(drive.textarea().value).toBe("refused");

    // The merge is on screen; a deliberate clear is final.
    await drive.type("");
    expect(drive.textarea().value).toBe("");
    await drive.type("hi");
    expect(drive.textarea().value).toBe("hi");
  });
});
