// The composer's file-attachment state: limits refused as chips, an upload
// moving to ready, removal aborting an in-flight upload, and a clear that
// only takes the ready rows.
// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AttachmentReference } from "../../types/ipc";
import { MAX_FILE_ATTACHMENT_BYTES, type FileUploader } from "./fileUpload";
import {
  FILES_UNSUPPORTED_REASON,
  MAX_COMPOSER_FILES,
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

function pdfFile(name = "report.pdf"): File {
  return new File([new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8])], name, {
    type: "application/pdf",
  });
}

function Probe(args: { sessionId?: string; uploader: FileUploader | null; supported?: boolean }) {
  api = useFileAttachments({
    sessionId: args.sessionId ?? "s.a.1",
    uploader: args.uploader,
    supported: args.supported ?? true,
  });
  return null;
}

async function render(args: { uploader: FileUploader | null; supported?: boolean }) {
  await act(async () => {
    root.render(<Probe uploader={args.uploader} supported={args.supported} />);
  });
}

async function add(...files: File[]) {
  await act(async () => {
    api!.addFiles(files);
  });
  await act(async () => {});
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
    const big = pdfFile("big.pdf");
    Object.defineProperty(big, "size", { value: MAX_FILE_ATTACHMENT_BYTES + 1 });
    await add(big);
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

  it("clears only the ready rows after a successful send", async () => {
    await render({ uploader: instantUploader() });
    const big = pdfFile("big.pdf");
    Object.defineProperty(big, "size", { value: MAX_FILE_ATTACHMENT_BYTES + 1 });
    await add(pdfFile(), big);
    expect(api!.files.map((file) => file.state)).toEqual(["ready", "refused"]);
    await act(async () => {
      api!.clearReady();
    });
    expect(api!.files.map((file) => file.state)).toEqual(["refused"]);
  });
});
