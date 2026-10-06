// The composer's file-attachment state: limits refused as chips, one upload at
// a time, removal aborting or releasing, and a clear that only takes the ready
// rows.
// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AttachmentReference } from "../../types/ipc";
import { MAX_FILE_ATTACHMENT_BYTES, MAX_UPLOAD_CHUNK_BYTES, type FileUploader } from "./fileUpload";
import {
  FILES_UNSUPPORTED_REASON,
  MAX_COMPOSER_FILES,
  MAX_COMPOSER_TOTAL_FILE_BYTES,
  useFileAttachments,
  type FileAttachments,
} from "./useFileAttachments";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;
let api: FileAttachments | null = null;

const REFERENCE: AttachmentReference = {
  sessionId: "s.a.1",
  digest: "a".repeat(64),
  storedBytes: 8,
  name: "report.pdf",
};

function instantUploader(overrides: Partial<FileUploader> = {}): FileUploader {
  return {
    begin: vi.fn(async () => 0),
    status: vi.fn(async () => 0),
    chunk: vi.fn(async (_session, _upload, offset, data) => offset + atob(data).length),
    finish: vi.fn(async () => REFERENCE),
    abort: vi.fn(async () => {}),
    ...overrides,
  };
}

function pdfFile(name = "report.pdf", size = 8): File {
  return new File([new Uint8Array(size)], name, { type: "application/pdf" });
}

function sizedFile(name: string, size: number): File {
  const file = new File([new Uint8Array(1)], name, { type: "application/pdf" });
  Object.defineProperty(file, "size", { value: size });
  return file;
}

function Probe(args: {
  sessionId?: string;
  uploader: FileUploader | null;
  supported?: boolean;
  remove?: (reference: AttachmentReference) => Promise<void>;
}) {
  api = useFileAttachments({
    sessionId: args.sessionId ?? "s.a.1",
    uploader: args.uploader,
    supported: args.supported ?? true,
    remove: args.remove ?? (async () => {}),
  });
  return null;
}

async function render(args: {
  uploader: FileUploader | null;
  supported?: boolean;
  remove?: (reference: AttachmentReference) => Promise<void>;
}) {
  await act(async () => {
    root.render(<Probe uploader={args.uploader} supported={args.supported} remove={args.remove} />);
  });
}

async function add(...files: File[]) {
  await act(async () => {
    api!.addFiles(files);
  });
  // One upload runs at a time, so each file needs its own settle.
  for (let index = 0; index < files.length + 1; index += 1) {
    await act(async () => {});
  }
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  api = null;
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

describe("useFileAttachments", () => {
  it("moves a file from uploading to ready with the daemon's reference", async () => {
    await render({ uploader: instantUploader() });
    await add(pdfFile());
    const [file] = api!.files;
    expect(file).toBeDefined();
    expect(file!.name).toBe("report.pdf");
    expect(file!.size).toBe(8);
    expect(file!.state).toBe("ready");
    expect(file!.reference).toEqual(REFERENCE);
    expect(api!.hasPending).toBe(false);
  });

  it("refuses a file over the per-file limit without uploading it", async () => {
    const uploader = instantUploader();
    await render({ uploader });
    await add(sizedFile("big.pdf", MAX_FILE_ATTACHMENT_BYTES + 1));
    expect(api!.files[0]!.state).toBe("refused");
    expect(api!.files[0]!.reason).toContain("50 MiB");
    expect(uploader.begin).not.toHaveBeenCalled();
    expect(api!.hasPending).toBe(true);
  });

  it("refuses every file when the daemon did not agree uploads", async () => {
    await render({ uploader: null, supported: false });
    await add(pdfFile());
    expect(api!.files[0]!.state).toBe("refused");
    expect(api!.files[0]!.reason).toBe(FILES_UNSUPPORTED_REASON);
  });

  it("refuses the ninth file with the count reason", async () => {
    await render({ uploader: instantUploader() });
    await add(
      ...Array.from({ length: MAX_COMPOSER_FILES + 1 }, (_, index) => pdfFile(`file-${index}.pdf`)),
    );
    expect(api!.files).toHaveLength(MAX_COMPOSER_FILES + 1);
    const last = api!.files[MAX_COMPOSER_FILES]!;
    expect(last.state).toBe("refused");
    expect(last.reason).toContain(String(MAX_COMPOSER_FILES));
  });

  it("does not let a refused chip spend the file budget", async () => {
    await render({ uploader: instantUploader() });
    await add(
      ...Array.from({ length: MAX_COMPOSER_FILES }, () =>
        sizedFile("huge.pdf", MAX_FILE_ATTACHMENT_BYTES + 1),
      ),
    );
    expect(api!.files.every((file) => file.state === "refused")).toBe(true);
    await add(pdfFile("real.pdf"));
    const accepted = api!.files.filter((file) => file.state !== "refused");
    expect(accepted.map((file) => file.name)).toEqual(["real.pdf"]);
  });

  it("bounds the chip list on an oversized drop", async () => {
    await render({ uploader: instantUploader() });
    await add(...Array.from({ length: 200 }, (_, index) => pdfFile(`file-${index}.pdf`)));
    expect(api!.files.length).toBeLessThanOrEqual(MAX_COMPOSER_FILES * 2);
    expect(api!.files.some((file) => file.state === "refused")).toBe(true);
  });

  it("refuses a pick that would pass the total cap", async () => {
    // The spoofed sizes walk the cap arithmetic without carrying real bytes;
    // the fake chunk reports a full chunk each time so the loop advances.
    const uploader = instantUploader({
      chunk: vi.fn(async (_session, _upload, offset) => offset + MAX_UPLOAD_CHUNK_BYTES),
    });
    await render({ uploader });
    const half = MAX_COMPOSER_TOTAL_FILE_BYTES / 2 + 1;
    await add(sizedFile("one.pdf", half), sizedFile("two.pdf", half));
    expect(uploader.begin).toHaveBeenCalledTimes(1);
    expect(api!.files[1]!.state).toBe("refused");
    expect(api!.files[1]!.reason).toContain("72 MiB");
  });

  it("runs one upload at a time", async () => {
    const releases: Array<() => void> = [];
    const uploader = instantUploader({
      begin: vi.fn(
        () =>
          new Promise<number>((resolve) => {
            releases.push(() => resolve(0));
          }),
      ),
      chunk: vi.fn(async (_session, _upload, offset, data) => offset + atob(data).length),
    });
    await render({ uploader });
    await add(pdfFile("a.pdf"), pdfFile("b.pdf"));
    expect(uploader.begin).toHaveBeenCalledTimes(1);
    await act(async () => {
      releases[0]!();
    });
    await act(async () => {});
    await act(async () => {});
    expect(uploader.begin).toHaveBeenCalledTimes(2);
  });

  it("aborts an in-flight upload when its chip is removed", async () => {
    const abort = vi.fn(async () => {});
    const uploader = instantUploader({
      begin: vi.fn(() => new Promise<number>(() => {})),
      abort,
    });
    await render({ uploader });
    await add(pdfFile());
    const id = api!.files[0]!.id;
    expect(api!.files[0]!.state).toBe("uploading");
    await act(async () => {
      api!.removeFile(id);
    });
    expect(abort).toHaveBeenCalledTimes(1);
    expect(api!.files).toHaveLength(0);
  });

  it("aborts the in-flight upload when the surface unmounts", async () => {
    const abort = vi.fn(async () => {});
    const uploader = instantUploader({
      begin: vi.fn(() => new Promise<number>(() => {})),
      abort,
    });
    await render({ uploader });
    await add(pdfFile());
    await act(async () => {
      root.unmount();
    });
    expect(abort).toHaveBeenCalledTimes(1);
    root = createRoot(container);
  });

  it("releases the hosted bytes when a ready chip is removed", async () => {
    const remove = vi.fn(async () => {});
    await render({ uploader: instantUploader(), remove });
    await add(pdfFile());
    await act(async () => {
      api!.removeFile(api!.files[0]!.id);
    });
    expect(remove).toHaveBeenCalledWith(REFERENCE);
    expect(api!.files).toHaveLength(0);
  });

  it("does not release bytes another ready chip still names", async () => {
    const remove = vi.fn(async () => {});
    await render({ uploader: instantUploader(), remove });
    await add(pdfFile("one.pdf"), pdfFile("two.pdf"));
    expect(api!.files.map((file) => file.state)).toEqual(["ready", "ready"]);
    await act(async () => {
      api!.removeFile(api!.files[0]!.id);
    });
    expect(remove).not.toHaveBeenCalled();
  });

  it("clears only the ready rows after a successful send", async () => {
    await render({ uploader: instantUploader() });
    await add(pdfFile(), sizedFile("big.pdf", MAX_FILE_ATTACHMENT_BYTES + 1));
    expect(api!.files.map((file) => file.state)).toEqual(["ready", "refused"]);
    await act(async () => {
      api!.clearReady();
    });
    expect(api!.files.map((file) => file.state)).toEqual(["refused"]);
  });
});
