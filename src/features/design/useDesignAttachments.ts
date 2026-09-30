import { useCallback, useRef, useState } from "react";
import type { DesignAttachment, DesignAttachmentFeedback } from "./designHost";
import {
  attachmentPillKey,
  attachmentReadFailureMessage,
  importDesignAttachments,
} from "./designAttachments";
import type { AttachmentMessage } from "./designSurfaceTypes";

interface UseDesignAttachmentsResult {
  attachments: readonly DesignAttachment[];
  attachmentMessages: readonly AttachmentMessage[];
  attachmentProgress: string | null;
  handleAttachFiles: (files: readonly File[], problem: string | null) => void;
  handleAttachmentProblem: (message: string) => void;
  handleRemoveAttachment: (key: string) => void;
  handleAttachmentFeedback: (message: DesignAttachmentFeedback) => void;
  consumeAttachments: () => void;
  clearAttachmentProgress: () => void;
  abortInFlightImport: () => void;
}

export function useDesignAttachments(): UseDesignAttachmentsResult {
  /**
   * Files imported as starting points for the run the user is about to start.
   * Deliberately not part of the document and never persisted: an attachment
   * belongs to the request it was attached to, and `startGeneration` clears both
   * this and the draft at once so the composer never shows a file that the run it
   * is describing did not carry.
   */
  const [attachments, setAttachments] = useState<readonly DesignAttachment[]>([]);
  const [attachmentMessages, setAttachmentMessages] = useState<readonly AttachmentMessage[]>([]);
  /**
   * The same list the state holds, and the one imports are measured against. The
   * importer's ceilings are computed from what is already attached, so an import
   * has to see the list as it stands when it runs, not as it stood when its handler
   * was created; and two imports must not measure against the same baseline and
   * together pass a ceiling neither would pass alone. Hence the queue: one import
   * at a time, each reading the ref the previous one finished writing.
   */
  const attachmentsRef = useRef<readonly DesignAttachment[]>([]);
  const attachQueueRef = useRef<Promise<void>>(Promise.resolve());
  /**
   * Bumped by every run that consumes the composer. An import reads a file
   * asynchronously against the list as it stood when the read began; if a run
   * empties that list while the read is in flight, the import's result describes
   * a composer that no longer exists. Without this, the resolved import writes
   * its files back after `startGeneration` cleared them, so a consumed file
   * reappears in the composer under a run that did not carry it. The epoch is
   * captured when the import starts and checked before it writes: an import
   * started before the consumption cannot write after it.
   */
  const attachmentEpochRef = useRef(0);
  /**
   * The import in flight, if any.
   *
   * It exists so that work which is no longer wanted can be stopped rather than
   * finished: a run that consumes the composer aborts it (the epoch above would
   * discard the result anyway, and a render is not a thing to spend on a
   * composer that has moved on), and unmounting aborts it too. The renderer
   * checks the signal between pages and inside a page's own render loop, so an
   * abort costs the page in flight, not the call.
   */
  const attachControllerRef = useRef<AbortController | null>(null);
  /**
   * What the import is doing right now, as a page count (`deck.pdf: page 2 of
   * 3.`). Live state rather than an import message: it describes work in
   * progress and has to leave when the work does.
   */
  const [attachmentProgress, setAttachmentProgress] = useState<string | null>(null);

  /**
   * The one place the two are written together. `setAttachments` alone would leave
   * the ref behind, and the ref is what the next import measures against.
   */
  const commitAttachments = useCallback((next: readonly DesignAttachment[]) => {
    attachmentsRef.current = next;
    setAttachments(next);
  }, []);

  /**
   * The composer's feedback about a run's attachments, from the host that
   * stores them.
   *
   * The pages of an attached document are deposited after this surface has
   * handed them over — the run clears the composer the moment it starts — so
   * this callback is the only route a progress count, or the sentence naming the
   * pages that did not make it, has back to the row they belong to. Progress
   * replaces the one transient line the import also uses; a note or an error is
   * kept, exactly as an import's own feedback is.
   */
  const handleAttachmentFeedback = useCallback((message: DesignAttachmentFeedback): void => {
    const { kind, text, detail } = message;
    if (kind === "progress") {
      setAttachmentProgress(text);
      return;
    }
    setAttachmentProgress(null);
    setAttachmentMessages((current) => [
      ...current,
      { kind, text, ...(detail === undefined ? {} : { detail }) },
    ]);
  }, []);

  const handleAttachFiles = useCallback(
    (files: readonly File[], problem: string | null) => {
      attachQueueRef.current = attachQueueRef.current.then(async () => {
        // Captured at the start of the read, checked before the write: a run
        // that consumed the composer in between has moved the epoch, and this
        // import's result — both the files and the feedback about them —
        // belongs to the composer it was measured against, not the one the run
        // left behind.
        const epoch = attachmentEpochRef.current;
        // The controller makes the import stoppable rather than only cancelable
        // in theory: a run that consumes the composer aborts it instead of
        // waiting for pages that run will discard, and unmounting aborts it too.
        // The progress line is the import's own page count and leaves with it.
        const controller = new AbortController();
        attachControllerRef.current = controller;
        setAttachmentProgress(null);
        // The whole body is guarded because this promise IS the queue: if it
        // rejects, `attachQueueRef.current` becomes a rejected promise and
        // every later `.then` on it silently skips its callback. One throw
        // would kill attaching for the rest of the session — the drop zone
        // would still light up and nothing would ever happen again. The import
        // reads files and lazily loads the PDF renderer, so throwing is not
        // hypothetical: a file moved between the picker and the read, or a
        // chunk that fails to load, both land here.
        let result: Awaited<ReturnType<typeof importDesignAttachments>>;
        try {
          result = await importDesignAttachments(files, attachmentsRef.current, {
            signal: controller.signal,
            onProgress: setAttachmentProgress,
          });
        } catch (cause) {
          if (attachmentEpochRef.current !== epoch) return;
          setAttachmentMessages([
            {
              kind: "error",
              text: attachmentReadFailureMessage(files, cause),
            },
          ]);
          return;
        } finally {
          if (attachControllerRef.current === controller) attachControllerRef.current = null;
          setAttachmentProgress(null);
        }
        if (attachmentEpochRef.current !== epoch) return;
        if (result.attachments.length > 0) {
          commitAttachments([...attachmentsRef.current, ...result.attachments]);
        }
        // Replaced wholesale, including by an empty list: the feedback describes
        // the last import, and an import that had nothing to say clears what the
        // one before it said.
        setAttachmentMessages([
          ...(problem === null ? [] : [{ kind: "error" as const, text: problem }]),
          ...result.rejections.map((rejection) => ({
            kind: "error" as const,
            text: rejection.reason,
          })),
          ...result.notices.map((notice) => ({ kind: "note" as const, text: notice })),
        ]);
      });
    },
    [commitAttachments],
  );

  const handleAttachmentProblem = useCallback(
    (message: string) => setAttachmentMessages([{ kind: "error", text: message }]),
    [],
  );

  const handleRemoveAttachment = useCallback(
    // The key belongs to a pill, and a document's pill key is the document: one
    // press takes every page of it, so a deck cannot be left with a page
    // missing. A page's own id is nobody's pill key, so asking to remove one
    // removes nothing rather than half a document.
    (key: string) =>
      commitAttachments(attachmentsRef.current.filter((item) => attachmentPillKey(item) !== key)),
    [commitAttachments],
  );

  // A run consumed the starting points: the composer is emptied, the epoch moves
  // so an in-flight import cannot write back into it, and the import itself is
  // stopped rather than finished.
  const consumeAttachments = useCallback(() => {
    attachmentEpochRef.current += 1;
    attachControllerRef.current?.abort();
    commitAttachments([]);
    setAttachmentMessages([]);
  }, [commitAttachments]);

  // The run's deposits are over, so its progress line goes with it. The
  // sentences about pages that did not make it stay in the composer: they are
  // the user's record of what the agent was handed.
  const clearAttachmentProgress = useCallback(() => {
    setAttachmentProgress(null);
  }, []);

  // Unmount calls this through the surface's mounted effect, which stays where
  // it is: it also owns `mountedRef`, which save and generation read.
  const abortInFlightImport = useCallback(() => {
    attachControllerRef.current?.abort();
  }, []);

  return {
    attachments,
    attachmentMessages,
    attachmentProgress,
    handleAttachFiles,
    handleAttachmentProblem,
    handleRemoveAttachment,
    handleAttachmentFeedback,
    consumeAttachments,
    clearAttachmentProgress,
    abortInFlightImport,
  };
}
