// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { invokeAgentCommand } from "./agentHost";
import {
  attachmentPillKey,
  DESIGN_DEPOSIT_BUDGET_BYTES,
  DESIGN_PDF_MAX_PAGES,
  MAX_ATTACHMENT_BYTES,
  transportDesignAttachments,
} from "./designAttachments";
import type { DesignAttachment, DesignAttachmentFeedback } from "./designHost";
import type { AttachmentReference } from "../../lib/tauri";
import type { PromptAttachment } from "../../types/ipc";

/**
 * The two commands the composer's send path uses, and only those: everything
 * else in `src/lib/tauri.ts` is the real module, so the argument shapes asserted
 * below are the ones the app really builds.
 */
const mocks = vi.hoisted(() => ({ sessionSend: vi.fn(), sessionDeposit: vi.fn() }));

vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/tauri")>();
  return {
    ...actual,
    sessionSend: mocks.sessionSend,
    sessionDeposit: mocks.sessionDeposit,
  };
});

const SESSION = "s.owner.1";

/** One page of a document, shaped exactly as `importDesignAttachments` builds it. */
function page(
  documentId: string,
  name: string,
  pageNumber: number,
  travelled: number,
  bytes: number,
): DesignAttachment {
  return {
    id: `${documentId}-p${pageNumber}`,
    kind: "raster",
    name: `${name} page ${pageNumber} of ${travelled}`,
    mimeType: "image/jpeg",
    bytes,
    base64: "AAAA",
    document: { id: documentId, name, page: pageNumber, pageCount: travelled, travelled },
  };
}

/** A whole document the composer carries: `pages` pictures of one file. */
function deck(
  pages: number,
  options: { readonly id?: string; readonly name?: string; readonly bytes?: number } = {},
): readonly DesignAttachment[] {
  const id = options.id ?? "doc-1";
  const name = options.name ?? "deck.pdf";
  const bytes = options.bytes ?? 512;
  return Array.from({ length: pages }, (_unused, index) => page(id, name, index + 1, pages, bytes));
}

/** The deposit as the controller performs it: through the same command seam. */
const depositThrough = (sessionId: string) => (attachment: PromptAttachment) =>
  invokeAgentCommand<AttachmentReference>("session_deposit", { id: sessionId, attachment });

/** The reference a daemon would answer with for this attachment, keyed by its name. */
function answered(id: string, attachment: PromptAttachment): AttachmentReference {
  return {
    sessionId: id,
    digest: `digest-${attachment.name}`,
    storedBytes: attachment.data.length,
  };
}

/** Every page is stored. */
function storeEveryPage(): void {
  mocks.sessionDeposit.mockImplementation((id: string, attachment: PromptAttachment) =>
    Promise.resolve(answered(id, attachment)),
  );
}

describe("a document's pages reach the prompt as deposits", () => {
  beforeEach(() => {
    mocks.sessionSend.mockReset();
    mocks.sessionDeposit.mockReset();
  });

  it("deposits forty pages in order, and the send names forty references", async () => {
    // Sequential is the decision, not an accident of the loop: the second frame
    // must not leave before the first was answered.
    let inFlight = 0;
    let mostInFlight = 0;
    mocks.sessionDeposit.mockImplementation(async (id: string, attachment: PromptAttachment) => {
      inFlight += 1;
      mostInFlight = Math.max(mostInFlight, inFlight);
      await Promise.resolve();
      inFlight -= 1;
      return answered(id, attachment);
    });

    const transport = await transportDesignAttachments({
      attachments: deck(DESIGN_PDF_MAX_PAGES),
      storedBytes: 0,
      deposit: depositThrough(SESSION),
    });

    expect(mocks.sessionDeposit).toHaveBeenCalledTimes(DESIGN_PDF_MAX_PAGES);
    expect(mostInFlight).toBe(1);
    expect(transport.inline).toEqual([]);
    expect(transport.notices).toEqual([]);
    expect(transport.refused).toBe(false);
    expect(transport.references).toHaveLength(DESIGN_PDF_MAX_PAGES);

    await invokeAgentCommand("session_send", {
      id: SESSION,
      subscriptionId: 41,
      text: "here is the deck",
      attachmentReferences: transport.references,
    });

    expect(mocks.sessionSend).toHaveBeenCalledTimes(1);
    // Nothing inline: every page of the document travelled in a deposit frame,
    // and none of them rode in the prompt.
    expect(mocks.sessionSend.mock.calls[0]?.[3]).toBeUndefined();
    const named = (mocks.sessionSend.mock.calls[0]?.[5] ?? []) as readonly AttachmentReference[];
    expect(mocks.sessionSend.mock.calls[0]?.[5]).toEqual(transport.references);
    expect(named.map((reference) => reference.digest)).toEqual(
      Array.from(
        { length: DESIGN_PDF_MAX_PAGES },
        (_unused, index) => `digest-deck.pdf page ${index + 1} of 40`,
      ),
    );
  });

  it("deposits a one-page document too, rather than giving it the inline path", async () => {
    storeEveryPage();

    const transport = await transportDesignAttachments({
      attachments: deck(1, { id: "doc-one", name: "one.pdf" }),
      storedBytes: 0,
      deposit: depositThrough(SESSION),
    });

    // One rule and not two: the round trip a one-page document would save by
    // riding inline is worth less than a single path through this function.
    expect(mocks.sessionDeposit).toHaveBeenCalledTimes(1);
    expect(transport.inline).toEqual([]);
    expect(transport.references).toHaveLength(1);
  });

  it("keeps the pages that made it, and names the ones that did not", async () => {
    mocks.sessionDeposit.mockImplementation((id: string, attachment: PromptAttachment) =>
      attachment.name.includes("page 12 of")
        ? Promise.reject(new Error("the store is full"))
        : Promise.resolve(answered(id, attachment)),
    );
    const feedback: DesignAttachmentFeedback[] = [];

    const transport = await transportDesignAttachments({
      attachments: deck(DESIGN_PDF_MAX_PAGES),
      storedBytes: 0,
      deposit: depositThrough(SESSION),
      onFeedback: (message) => feedback.push(message),
    });

    // The sequence stops at the page the store refused: the eleven pages before
    // it were never retried, and the twenty-eight after it were never attempted.
    expect(mocks.sessionDeposit).toHaveBeenCalledTimes(12);
    expect(transport.refused).toBe(false);
    expect(transport.references).toHaveLength(11);
    expect(transport.references.at(-1)?.digest).toBe("digest-deck.pdf page 11 of 40");
    expect(transport.notices).toEqual([
      "deck.pdf was stored in part: pages 12-40 were left out because the store is full, so 11 of its 40 pages travel with this prompt.",
    ]);
    // The pill row: a count per page while the store works, then the sentence.
    expect(feedback.filter((message) => message.kind === "progress").map((m) => m.text)).toEqual(
      Array.from({ length: 11 }, (_unused, index) => `deck.pdf: page ${index + 1} of 40.`),
    );
    expect(feedback.filter((message) => message.kind === "error").map((m) => m.text)).toEqual(
      transport.notices,
    );
  });

  it("names the document after the one that failed, which was never attempted", async () => {
    mocks.sessionDeposit.mockImplementation((id: string, attachment: PromptAttachment) =>
      attachment.name.includes("deck.pdf page 2 of")
        ? Promise.reject(new Error("the store is full"))
        : Promise.resolve(answered(id, attachment)),
    );

    const transport = await transportDesignAttachments({
      attachments: [
        ...deck(3, { id: "doc-a", name: "deck.pdf" }),
        ...deck(2, { id: "doc-b", name: "notes.pdf" }),
      ],
      storedBytes: 0,
      deposit: depositThrough(SESSION),
    });

    expect(transport.references).toHaveLength(1);
    expect(transport.notices).toEqual([
      "deck.pdf was stored in part: pages 2-3 were left out because the store is full, so 1 of its 3 pages travels with this prompt.",
      "notes.pdf was not stored: pages 1-2 were left out because the deposit stopped at deck.pdf page 2, so none of its pages travel with this prompt.",
    ]);
  });

  it("refuses a plan the store cannot take before the first deposit", async () => {
    storeEveryPage();

    const transport = await transportDesignAttachments({
      attachments: deck(DESIGN_PDF_MAX_PAGES, { bytes: MAX_ATTACHMENT_BYTES }),
      // 5 MiB of pages against a store with a kilobyte left: nothing fits.
      storedBytes: DESIGN_DEPOSIT_BUDGET_BYTES - 1024,
      deposit: depositThrough(SESSION),
    });

    // Refused before a frame left: the sentence the composer shows is the one
    // the import uses for the same bound, with the send-time action attached.
    expect(mocks.sessionDeposit).not.toHaveBeenCalled();
    expect(transport.refused).toBe(true);
    expect(transport.references).toEqual([]);
    expect(transport.inline).toEqual([]);
    expect(transport.notices).toEqual([
      "deck.pdf has 40 pages, and none of them fits: one rendered page needs up to 96.0 KB and the attachment store has 1.0 KB free, so nothing was attached. Remove an attached file and send again.",
    ]);
  });

  it("still carries a dropped picture inline, and deposits nothing for it", async () => {
    storeEveryPage();
    const picture: DesignAttachment = {
      id: "att-1",
      kind: "raster",
      name: "shot.png",
      mimeType: "image/png",
      bytes: 3,
      base64: "AAAA",
    };

    const transport = await transportDesignAttachments({
      attachments: [picture],
      storedBytes: 0,
      deposit: depositThrough(SESSION),
    });

    expect(mocks.sessionDeposit).not.toHaveBeenCalled();
    expect(transport.inline).toEqual([picture]);
    expect(transport.references).toEqual([]);
    expect(transport.notices).toEqual([]);
    expect(transport.refused).toBe(false);
  });

  it("shows one pill for the whole document, not forty attachments", () => {
    const pages = deck(DESIGN_PDF_MAX_PAGES);

    expect(new Set(pages.map((attachment) => attachmentPillKey(attachment))).size).toBe(1);
    expect(attachmentPillKey(pages[0])).toBe("doc-1");
  });
});
