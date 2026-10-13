import { useEffect, useState } from "react";
import type { ChatFileLinks, ChatImageBytes } from "../../../lib/chatFilePaths";
import { workspaceRelativePath } from "../../../lib/workspaceRelativePath";
import { ImageLightbox } from "./ImageLightbox";
import { useNearViewport } from "./useNearViewport";

/** Reads one workspace-relative picture; a refusal is null. */
export type ImageReader = (relative: string) => Promise<ChatImageBytes | null>;

/**
 * A reader that asks the workspace once per path and hands every later
 * request the same answer, so a picture an answer repeats is read once.
 */
export function sharedImageReader(readImage: NonNullable<ChatFileLinks["readImage"]>): ImageReader {
  const reads = new Map<string, Promise<ChatImageBytes | null>>();
  return (relative) => {
    let read = reads.get(relative);
    if (read === undefined) {
      read = readImage(relative);
      reads.set(relative, read);
    }
    return read;
  };
}

/** base64 as the blob URL an `<img>` shows; null when it does not decode. */
function blobUrl(mimeType: string, data: string): string | null {
  try {
    const bytes = Uint8Array.from(atob(data), (char) => char.charCodeAt(0));
    return URL.createObjectURL(new Blob([bytes], { type: mimeType }));
  } catch {
    return null;
  }
}

/**
 * One `![alt](path)` the agent wrote, drawn as the same thumbnail a tool
 * result's picture is. The path must resolve inside the workspace root, and
 * the bytes come from the workspace's own file read, which re-confines the
 * relative path to the workspace and refuses links and junctions out of the
 * tree; a path outside it stays the alt text — never an arbitrary-path read.
 * The read waits until the picture is near the viewport. Pressing the picture
 * opens the transcript's viewer.
 */
export function MarkdownImage({
  target,
  alt,
  links,
  read,
}: {
  target: string;
  alt: string;
  links: ChatFileLinks;
  read: ImageReader;
}) {
  const relative = workspaceRelativePath(target, links.root);
  const [element, setElement] = useState<HTMLElement | null>(null);
  const near = useNearViewport(element);
  const [url, setUrl] = useState<string | null | undefined>(undefined);
  const [opener, setOpener] = useState<HTMLElement | null>(null);

  useEffect(() => {
    if (relative === null || !near) return;
    let revoked = false;
    let objectUrl: string | null = null;
    read(relative).then(
      (bytes) => {
        if (revoked) return;
        objectUrl = bytes === null ? null : blobUrl(bytes.mimeType, bytes.data);
        setUrl(objectUrl);
      },
      () => {
        if (!revoked) setUrl(null);
      },
    );
    return () => {
      revoked = true;
      if (objectUrl !== null) URL.revokeObjectURL(objectUrl);
    };
  }, [relative, read, near]);

  if (relative === null) {
    return <span className="plan-markdown-image">{alt === "" ? "image" : alt}</span>;
  }
  if (url === undefined) {
    return <span ref={setElement} className="workspace-chat-image-loading" aria-hidden="true" />;
  }
  if (url === null) {
    return (
      <span
        className="workspace-chat-image-unavailable"
        role="img"
        aria-label={`${alt} unavailable`}
      >
        Image unavailable
      </span>
    );
  }
  const label = alt === "" ? "Image" : alt;
  return (
    <span className="workspace-chat-images">
      <button
        type="button"
        className="workspace-chat-image-thumb"
        aria-label={label}
        onClick={(event) => setOpener(event.currentTarget)}
      >
        <img src={url} alt="" />
      </button>
      {opener === null ? null : (
        <ImageLightbox
          images={[{ url, alt: label }]}
          index={0}
          openedIndex={0}
          openedUrl={url}
          opener={opener}
          onIndexChange={() => undefined}
          onClose={() => setOpener(null)}
        />
      )}
    </span>
  );
}
