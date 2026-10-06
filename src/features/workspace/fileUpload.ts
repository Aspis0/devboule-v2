import { base64Of } from "../../lib/base64";
import type { AttachmentReference } from "../../types/ipc";

/**
 * Largest raw slice one chunk frame carries: the wire's own chunk ceiling,
 * stated on this side so the split and the daemon's check agree by
 * construction.
 */
export const MAX_UPLOAD_CHUNK_BYTES = 256 * 1024;

/** Largest file the composer attaches (50 MiB): the daemon's `MAX_UPLOAD_BYTES`. */
export const MAX_FILE_ATTACHMENT_BYTES = 50 * 1024 * 1024;

/**
 * How long one upload frame may take before the upload fails.
 *
 * A hung IPC call is a promise that never settles: without this deadline the
 * one-at-a-time queue would park forever behind it, and Send would stay
 * disabled for the session. The timeout fails *this* upload with a visible
 * reason, so the queue moves on to the next file.
 */
export const UPLOAD_CALL_TIMEOUT_MS = 30_000;

/**
 * The five frames one upload is made of, as the app can call them. The Tauri
 * wrappers are one implementation; a test's fake is another.
 */
export interface FileUploader {
  begin(sessionId: string, uploadId: string, name: string, totalBytes: number): Promise<number>;
  status(sessionId: string, uploadId: string): Promise<number>;
  chunk(sessionId: string, uploadId: string, offset: number, data: string): Promise<number>;
  finish(sessionId: string, uploadId: string): Promise<AttachmentReference>;
  abort(sessionId: string, uploadId: string): Promise<void>;
}

/**
 * Upload one file by reference and answer the daemon's reference to it.
 *
 * The bytes never leave in one frame: `begin` states the size and answers the
 * offset the upload stands at, then each at-most-256 KiB slice is appended at
 * exactly that offset and the answer moves it forward. A chunk that fails is
 * not retried blind — the daemon's append is strict, so the uploader asks
 * `status` once for where the bytes actually ended, and only when that read
 * fails too does it `begin` again under the same id (a re-open the daemon
 * cannot match drops the declaration) and continue from that answer. `finish`
 * closes it and answers the reference a later send names.
 *
 * `signal` is checked before every chunk and before `finish`: an aborted
 * upload stops sending and rejects with an `AbortError`, and the caller is what
 * tells the daemon to drop the staged bytes. A failed chunk after an abort is
 * not a reason to resume — the re-`begin` a resume would issue is exactly the
 * "start over and store it anyway" this must not do. Every call is also raced
 * against the signal and [`UPLOAD_CALL_TIMEOUT_MS`], so a hung IPC frame
 * rejects instead of parking the queue.
 *
 * The file is read slice by slice, never whole: even a 50 MiB file costs one
 * chunk of transient memory here. A file the caller already refused for its
 * size is never read at all.
 */
export async function uploadFile(
  file: File,
  sessionId: string,
  uploadId: string,
  uploader: FileUploader,
  signal?: AbortSignal,
): Promise<AttachmentReference> {
  const throwIfAborted = () => {
    if (signal?.aborted) throw abortError();
  };
  throwIfAborted();
  const call = <T>(promise: Promise<T>) => raced(promise, UPLOAD_CALL_TIMEOUT_MS, signal);
  const declare = () => call(uploader.begin(sessionId, uploadId, file.name, file.size));
  let offset = await declare();
  // A chunk that does not move the offset is a retry, not progress; a daemon
  // that keeps answering the same number must not spin this loop forever.
  let stalls = 0;
  while (offset < file.size) {
    throwIfAborted();
    const slice = file.slice(offset, Math.min(offset + MAX_UPLOAD_CHUNK_BYTES, file.size));
    const data = base64Of(new Uint8Array(await slice.arrayBuffer()));
    let received: number;
    try {
      received = await call(uploader.chunk(sessionId, uploadId, offset, data));
    } catch (error: unknown) {
      if (signal?.aborted) throw abortError();
      // A frame that never came back is not a transient refusal to resume
      // past: the same call would hang again, and the queue would park.
      if (error instanceof Error && error.name === "TimeoutError") throw error;
      received = await resume(uploader, sessionId, uploadId, declare, call);
    }
    if (received <= offset) {
      stalls += 1;
      if (stalls > 2) throw new Error("The upload stopped making progress.");
    } else {
      stalls = 0;
    }
    offset = received;
  }
  throwIfAborted();
  return call(uploader.finish(sessionId, uploadId));
}

/**
 * Where the upload really stands after a chunk failed: the daemon's own answer
 * if it has one, and a fresh declaration under the same id when even the
 * `status` read failed. A daemon that answers an offset the chunk did not move
 * is taken at its word — the loop's next slice starts there.
 */
async function resume(
  uploader: FileUploader,
  sessionId: string,
  uploadId: string,
  declare: () => Promise<number>,
  call: <T>(promise: Promise<T>) => Promise<T>,
): Promise<number> {
  try {
    return await call(uploader.status(sessionId, uploadId));
  } catch {
    return declare();
  }
}

function abortError(): Error {
  const error = new Error("The upload was cancelled.");
  error.name = "AbortError";
  return error;
}

/**
 * One frame's promise, bounded by the deadline and the caller's abort.
 *
 * The underlying IPC call cannot be cancelled once sent, so this only decides
 * which rejection the loop sees first; the late settlement is still observed
 * by these handlers, so it can never surface as an unhandled rejection.
 */
function raced<T>(promise: Promise<T>, timeoutMs: number, signal?: AbortSignal): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    if (signal?.aborted) {
      reject(abortError());
      return;
    }
    let settled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const finish = (error: unknown, value?: T) => {
      if (settled) return;
      settled = true;
      if (timer !== null) clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
      if (error !== null) reject(error);
      else resolve(value as T);
    };
    const onAbort = () => finish(abortError());
    const timeoutError = new Error(`The upload call timed out after ${timeoutMs} ms.`);
    timeoutError.name = "TimeoutError";
    timer = setTimeout(() => finish(timeoutError), timeoutMs);
    signal?.addEventListener("abort", onAbort, { once: true });
    promise.then(
      (value) => finish(null, value),
      (error: unknown) => finish(error),
    );
  });
}
