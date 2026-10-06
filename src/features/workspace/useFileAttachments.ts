import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorSentence } from "../../lib/errorSentence";
import type { AttachmentReference } from "../../types/ipc";
import { MAX_FILE_ATTACHMENT_BYTES, uploadFile, type FileUploader } from "./fileUpload";

/** Most files one message carries: the daemon's `MAX_UPLOADED_FILES`. */
export const MAX_COMPOSER_FILES = 8;

/**
 * Most bytes one message's files may add up to: the daemon's whole owner
 * budget (`MAX_ATTACHMENT_OWNER_BYTES`). Without it eight 50 MiB picks stream
 * 400 MiB to a 72 MiB store and the last ones are refused only after the wire
 * has carried them.
 */
export const MAX_COMPOSER_TOTAL_FILE_BYTES = 72 * 1024 * 1024;

/** Said where the daemon's handshake did not include `attachments.upload`. */
export const FILES_UNSUPPORTED_REASON = "This daemon cannot receive files; update it first.";

/** One attached file as the composer shows it. `reason` is the daemon's own
 * refusal, verbatim, and only a `"refused"` entry has one. */
export interface AttachedFile {
  id: string;
  name: string;
  size: number;
  state: "uploading" | "ready" | "refused";
  reason?: string;
  reference?: AttachmentReference;
}

export interface FileAttachments {
  files: readonly AttachedFile[];
  addFiles: (picked: readonly File[]) => void;
  removeFile: (id: string) => void;
  clearReady: () => void;
  /** A chip that is not ready — an upload in flight, or a refusal waiting to be
   * removed — and therefore a send that must wait. */
  hasPending: boolean;
}

/**
 * Composer file attachments: the chips, their upload state, and the references
 * a send names.
 *
 * Files upload one at a time, in bounded chunks, and an entry moves to `ready`
 * with the daemon's reference or to `refused` with the daemon's own sentence. A
 * refusal is a chip the user removes, never a silent drop, which is why
 * `hasPending` counts refused entries too.
 *
 * A refused chip does not spend the file budget: it is not going to be sent, so
 * measuring the room against it would refuse a real file while seven dead chips
 * sit there. The chip list itself is bounded at twice the file count — accepted
 * chips first, then the newest refusals, so a two-hundred-file drop leaves a
 * readable composer and one summary refusal.
 */
export function useFileAttachments(args: {
  sessionId: string;
  uploader: FileUploader | null;
  supported: boolean;
  /** Releases the hosted bytes of one stored attachment. */
  remove: (reference: AttachmentReference) => Promise<void>;
}): FileAttachments {
  const [files, setFiles] = useState<readonly AttachedFile[]>([]);
  // The list as the last call left it, so two adds in one tick cannot both
  // measure the room against the same stale length.
  const filesRef = useRef<readonly AttachedFile[]>(files);
  const nextIdRef = useRef(0);
  const uploadIdsRef = useRef(new Map<string, string>());
  const controllersRef = useRef(new Map<string, AbortController>());
  // One upload runs at a time; the queue is a backstop against eight chunk
  // loops and eight base64 payloads in flight together.
  const pendingRef = useRef<Array<{ id: string; file: File; uploadId: string }>>([]);
  const activeRef = useRef(false);
  const disposedRef = useRef(false);
  const argsRef = useRef(args);
  argsRef.current = args;

  const publish = useCallback((next: readonly AttachedFile[]) => {
    filesRef.current = next;
    setFiles(next);
  }, []);

  const patch = useCallback(
    (id: string, change: Partial<AttachedFile>) => {
      if (disposedRef.current) return;
      publish(filesRef.current.map((entry) => (entry.id === id ? { ...entry, ...change } : entry)));
    },
    [publish],
  );

  const forgetUpload = useCallback((id: string) => {
    uploadIdsRef.current.delete(id);
    controllersRef.current.delete(id);
  }, []);

  /** Drop a part the daemon may still hold, best effort: the chip is going away
   * whether or not the abort lands. */
  const abortUpload = useCallback(
    (id: string) => {
      const uploadId = uploadIdsRef.current.get(id);
      const { sessionId, uploader } = argsRef.current;
      controllersRef.current.get(id)?.abort();
      forgetUpload(id);
      if (uploadId !== undefined && uploader !== null) {
        void uploader.abort(sessionId, uploadId).catch(() => undefined);
      }
    },
    [forgetUpload],
  );

  const pumpRef = useRef<() => void>(() => {});
  const pump = useCallback(() => {
    if (activeRef.current || disposedRef.current) return;
    let next = pendingRef.current.shift();
    // A pending chip the user removed is not uploaded: the daemon holds a
    // tombstone for its id, and a frame here would only be refused.
    while (next !== undefined) {
      const candidate = next;
      if (filesRef.current.some((entry) => entry.id === candidate.id)) break;
      next = pendingRef.current.shift();
    }
    if (next === undefined) return;
    const { sessionId, uploader } = argsRef.current;
    if (uploader === null) return;
    activeRef.current = true;
    const controller = new AbortController();
    controllersRef.current.set(next.id, controller);
    void uploadFile(next.file, sessionId, next.uploadId, uploader, controller.signal)
      .then(
        (reference) => patch(next.id, { state: "ready", reference }),
        (cause: unknown) => {
          // An abort is the chip being removed, not a refusal; the daemon drop
          // was already asked for by `abortUpload` or the unmount cleanup.
          if (controller.signal.aborted) return;
          abortUpload(next.id);
          patch(next.id, { state: "refused", reason: errorSentence(cause).sentence });
        },
      )
      .finally(() => {
        activeRef.current = false;
        pumpRef.current();
      });
  }, [abortUpload, patch]);
  pumpRef.current = pump;

  useEffect(() => {
    disposedRef.current = false;
    // The map and the queue are one instance for the hook's life; the cleanup
    // reads them directly rather than through the ref the lint warns about.
    const controllers = controllersRef.current;
    const pending = pendingRef.current;
    return () => {
      disposedRef.current = true;
      for (const id of controllers.keys()) {
        abortUpload(id);
      }
      pending.length = 0;
    };
  }, [abortUpload]);

  const addFiles = useCallback(
    (picked: readonly File[]) => {
      const { supported } = argsRef.current;
      const previous = filesRef.current;
      const acceptedBefore = previous.filter((entry) => entry.state !== "refused");
      let acceptedCount = acceptedBefore.length;
      let totalBytes = acceptedBefore.reduce((sum, entry) => sum + entry.size, 0);
      const accepted: AttachedFile[] = [];
      const refusals: AttachedFile[] = [];
      let overflow = 0;
      for (const file of picked) {
        nextIdRef.current += 1;
        const id = `f${Date.now().toString(36)}-${nextIdRef.current}`;
        if (acceptedCount >= MAX_COMPOSER_FILES) {
          overflow += 1;
          continue;
        }
        const reason = !supported
          ? FILES_UNSUPPORTED_REASON
          : file.size > MAX_FILE_ATTACHMENT_BYTES
            ? `${file.name} is larger than 50 MiB.`
            : totalBytes + file.size > MAX_COMPOSER_TOTAL_FILE_BYTES
              ? `The attached files add up to more than 72 MiB; ${file.name} was not attached.`
              : null;
        if (reason !== null || argsRef.current.uploader === null) {
          refusals.push({
            id,
            name: file.name,
            size: file.size,
            state: "refused",
            reason: reason ?? FILES_UNSUPPORTED_REASON,
          });
          continue;
        }
        const uploadId = `u${id}`;
        uploadIdsRef.current.set(id, uploadId);
        pendingRef.current.push({ id, file, uploadId });
        accepted.push({ id, name: file.name, size: file.size, state: "uploading" });
        acceptedCount += 1;
        totalBytes += file.size;
      }
      if (overflow > 0) {
        refusals.push({
          id: `f${Date.now().toString(36)}-${(nextIdRef.current += 1)}`,
          name: `${overflow} file${overflow === 1 ? "" : "s"}`,
          size: 0,
          state: "refused",
          reason: `${overflow} file${overflow === 1 ? " was" : "s were"} not attached: the composer carries at most ${MAX_COMPOSER_FILES} files.`,
        });
      }
      const refusalsKept = [
        ...previous.filter((entry) => entry.state === "refused"),
        ...refusals,
      ].slice(-MAX_COMPOSER_FILES);
      if (accepted.length > 0 || refusalsKept.length > 0) {
        publish([...acceptedBefore, ...accepted, ...refusalsKept]);
      }
      pump();
    },
    [publish, pump],
  );

  const removeFile = useCallback(
    (id: string) => {
      const entry = filesRef.current.find((candidate) => candidate.id === id);
      const { remove } = argsRef.current;
      if (entry === undefined) return;
      if (entry.state === "ready" && entry.reference !== undefined) {
        // The daemon keeps the bytes until asked; only a digest no other ready
        // chip still names is released, or removing one of two identical picks
        // would strand the other.
        const shared = filesRef.current.some(
          (candidate) =>
            candidate.id !== id &&
            candidate.state === "ready" &&
            candidate.reference?.digest === entry.reference?.digest,
        );
        if (!shared) void remove(entry.reference).catch(() => undefined);
        forgetUpload(id);
      } else {
        abortUpload(id);
      }
      publish(filesRef.current.filter((candidate) => candidate.id !== id));
    },
    [abortUpload, forgetUpload, publish],
  );

  const clearReady = useCallback(() => {
    for (const entry of filesRef.current) {
      if (entry.state === "ready") forgetUpload(entry.id);
    }
    publish(filesRef.current.filter((entry) => entry.state !== "ready"));
  }, [forgetUpload, publish]);

  return useMemo(
    () => ({
      files,
      addFiles,
      removeFile,
      clearReady,
      hasPending: files.some((entry) => entry.state !== "ready"),
    }),
    [addFiles, clearReady, files, removeFile],
  );
}
