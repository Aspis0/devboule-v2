import { useCallback, useMemo, useRef, useState } from "react";
import { errorSentence } from "../../lib/errorSentence";
import type { AttachmentReference } from "../../types/ipc";
import { MAX_FILE_ATTACHMENT_BYTES, uploadFile, type FileUploader } from "./fileUpload";

/** Most files one message carries: the daemon's `MAX_UPLOADED_FILES`. */
export const MAX_COMPOSER_FILES = 8;

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
 * A file starts uploading as soon as it is added, in bounded chunks, and its
 * entry moves to `ready` with the daemon's reference or to `refused` with the
 * daemon's own sentence. A refusal is a chip the user removes, never a silent
 * drop, which is why `hasPending` counts refused entries too.
 */
export function useFileAttachments(args: {
  sessionId: string;
  uploader: FileUploader | null;
  supported: boolean;
}): FileAttachments {
  const [files, setFiles] = useState<readonly AttachedFile[]>([]);
  // The list as the last call left it, so two adds in one tick cannot both
  // measure the room against the same stale length.
  const filesRef = useRef<readonly AttachedFile[]>(files);
  const nextIdRef = useRef(0);
  const uploadIdsRef = useRef(new Map<string, string>());
  const argsRef = useRef(args);
  argsRef.current = args;

  const publish = useCallback((next: readonly AttachedFile[]) => {
    filesRef.current = next;
    setFiles(next);
  }, []);

  const patch = useCallback(
    (id: string, change: Partial<AttachedFile>) => {
      publish(filesRef.current.map((entry) => (entry.id === id ? { ...entry, ...change } : entry)));
    },
    [publish],
  );

  const start = useCallback(
    (id: string, file: File, uploadId: string, sessionId: string, uploader: FileUploader) => {
      void uploadFile(file, sessionId, uploadId, uploader).then(
        (reference) => patch(id, { state: "ready", reference }),
        (cause: unknown) => patch(id, { state: "refused", reason: errorSentence(cause).sentence }),
      );
    },
    [patch],
  );

  const addFiles = useCallback(
    (picked: readonly File[]) => {
      const { sessionId, uploader, supported } = argsRef.current;
      const added: AttachedFile[] = [];
      let room = Math.max(0, MAX_COMPOSER_FILES - filesRef.current.length);
      for (const file of picked) {
        nextIdRef.current += 1;
        const id = `f${Date.now().toString(36)}-${nextIdRef.current}`;
        if (room <= 0) {
          added.push({
            id,
            name: file.name,
            size: file.size,
            state: "refused",
            reason: `${file.name} was not attached: the composer carries at most ${MAX_COMPOSER_FILES} files.`,
          });
          continue;
        }
        room -= 1;
        if (file.size > MAX_FILE_ATTACHMENT_BYTES) {
          added.push({
            id,
            name: file.name,
            size: file.size,
            state: "refused",
            reason: `${file.name} is larger than 50 MiB.`,
          });
          continue;
        }
        if (!supported || uploader === null) {
          added.push({
            id,
            name: file.name,
            size: file.size,
            state: "refused",
            reason: FILES_UNSUPPORTED_REASON,
          });
          continue;
        }
        added.push({ id, name: file.name, size: file.size, state: "uploading" });
        const uploadId = `u${id}`;
        uploadIdsRef.current.set(id, uploadId);
        start(id, file, uploadId, sessionId, uploader);
      }
      if (added.length > 0) publish([...filesRef.current, ...added]);
    },
    [publish, start],
  );

  const removeFile = useCallback(
    (id: string) => {
      const entry = filesRef.current.find((candidate) => candidate.id === id);
      const uploadId = uploadIdsRef.current.get(id);
      const { sessionId, uploader } = argsRef.current;
      uploadIdsRef.current.delete(id);
      if (entry?.state === "uploading" && uploadId !== undefined && uploader !== null) {
        void uploader.abort(sessionId, uploadId).catch(() => undefined);
      }
      publish(filesRef.current.filter((candidate) => candidate.id !== id));
    },
    [publish],
  );

  const clearReady = useCallback(() => {
    publish(filesRef.current.filter((entry) => entry.state !== "ready"));
  }, [publish]);

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
