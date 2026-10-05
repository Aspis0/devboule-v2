// The chunked uploader: split at the wire's ceiling, resume from the daemon's
// own offset after a failed chunk, start over when even that read fails.
// @vitest-environment happy-dom

import { describe, expect, it, vi } from "vitest";
import type { AttachmentReference } from "../../types/ipc";
import { MAX_UPLOAD_CHUNK_BYTES, uploadFile, type FileUploader } from "./fileUpload";

const REFERENCE: AttachmentReference = {
  sessionId: "s.a.1",
  digest: "a".repeat(64),
  storedBytes: MAX_UPLOAD_CHUNK_BYTES + 10,
};

function fileOf(size: number, name = "report.bin"): File {
  return new File([new Uint8Array(size).map((_, index) => index % 251)], name);
}

function rawLength(data: string): number {
  return atob(data).length;
}

describe("uploadFile", () => {
  it("splits the file at the wire's chunk ceiling and finishes in order", async () => {
    const size = MAX_UPLOAD_CHUNK_BYTES + 10;
    const file = fileOf(size);
    const chunks: Array<{ offset: number; raw: number }> = [];
    const uploader: FileUploader = {
      begin: vi.fn(async () => 0),
      status: vi.fn(async () => 0),
      chunk: vi.fn(async (_session, _upload, offset, data) => {
        chunks.push({ offset, raw: rawLength(data) });
        return offset + rawLength(data);
      }),
      finish: vi.fn(async () => ({ ...REFERENCE, storedBytes: size })),
      abort: vi.fn(async () => {}),
    };

    await expect(uploadFile(file, "s.a.1", "up-1", uploader)).resolves.toEqual({
      ...REFERENCE,
      storedBytes: size,
    });
    expect(chunks.map((chunk) => chunk.offset)).toEqual([0, MAX_UPLOAD_CHUNK_BYTES]);
    expect(chunks.map((chunk) => chunk.raw)).toEqual([MAX_UPLOAD_CHUNK_BYTES, 10]);
    expect(uploader.begin).toHaveBeenCalledTimes(1);
    expect(uploader.status).not.toHaveBeenCalled();
  });

  it("resumes from the offset the daemon reports after a failed chunk", async () => {
    const file = fileOf(2 * MAX_UPLOAD_CHUNK_BYTES);
    const offsets: number[] = [];
    let failOnce = true;
    const uploader: FileUploader = {
      begin: vi.fn(async () => 0),
      status: vi.fn(async () => MAX_UPLOAD_CHUNK_BYTES),
      chunk: vi.fn(async (_session, _upload, offset, data) => {
        if (failOnce && offset === MAX_UPLOAD_CHUNK_BYTES) {
          failOnce = false;
          throw new Error("connection reset");
        }
        offsets.push(offset);
        return offset + rawLength(data);
      }),
      finish: vi.fn(async () => REFERENCE),
      abort: vi.fn(async () => {}),
    };

    await uploadFile(file, "s.a.1", "up-1", uploader);
    expect(offsets).toEqual([0, MAX_UPLOAD_CHUNK_BYTES]);
    expect(uploader.begin).toHaveBeenCalledTimes(1);
    expect(uploader.status).toHaveBeenCalledTimes(1);
  });

  it("starts over under the same id when the status cannot answer", async () => {
    const file = fileOf(MAX_UPLOAD_CHUNK_BYTES);
    let begins = 0;
    let fails = 1;
    const uploader: FileUploader = {
      begin: vi.fn(async () => {
        begins += 1;
        return 0;
      }),
      status: vi.fn(async () => {
        throw new Error("not in progress");
      }),
      chunk: vi.fn(async (_session, _upload, offset, data) => {
        if (fails > 0) {
          fails -= 1;
          throw new Error("connection reset");
        }
        return offset + rawLength(data);
      }),
      finish: vi.fn(async () => REFERENCE),
      abort: vi.fn(async () => {}),
    };

    await uploadFile(file, "s.a.1", "up-1", uploader);
    expect(begins).toBe(2);
    expect(uploader.status).toHaveBeenCalledTimes(1);
  });

  it("surfaces a finish refusal with the daemon's own sentence", async () => {
    const file = fileOf(64);
    const uploader: FileUploader = {
      begin: vi.fn(async () => 0),
      status: vi.fn(async () => 0),
      chunk: vi.fn(async (_session, _upload, offset, data) => offset + rawLength(data)),
      finish: vi.fn(async () => {
        throw new Error("This store is full.");
      }),
      abort: vi.fn(async () => {}),
    };

    await expect(uploadFile(file, "s.a.1", "up-1", uploader)).rejects.toThrow(
      "This store is full.",
    );
  });
});
