import { useRef } from "react";
import { base64Of } from "../../lib/base64";
import type { PromptAttachment } from "../../types/ipc";
import { ComposerFileChips } from "./ComposerFileChips";
import type { AttachedFile } from "./useFileAttachments";

/**
 * Largest raw image the picker reads (128 KiB): the wire's per-attachment
 * base64 ceiling decodes to this, so a bigger file is refused by the deposit
 * and reading it first only wastes the memory. The daemon stays the
 * authority; this is the pick-time sentence, not the bound.
 */
const MAX_CHAT_IMAGE_BYTES = 128 * 1024;

/**
 * Most images one composer send carries (4): the wire's per-send attachment
 * bound, stated on this side so the fifth pick is refused with a sentence
 * instead of a daemon round trip. The composer enforces the same bound when
 * a handed-back row lands beside current picks.
 */
export const MAX_COMPOSER_IMAGES = 4;

const BASE_MIME_TYPES = ["image/png", "image/jpeg", "image/svg+xml"] as const;
/** Offered only to a daemon that agreed `attachments.gif_webp`. */
const GIF_WEBP_MIME_TYPES = ["image/gif", "image/webp"] as const;
type AcceptedMimeType = (typeof BASE_MIME_TYPES)[number] | (typeof GIF_WEBP_MIME_TYPES)[number];

const WITH_GIF_WEBP_MIME_TYPES: readonly AcceptedMimeType[] = [
  ...BASE_MIME_TYPES,
  ...GIF_WEBP_MIME_TYPES,
];

/** The image types this composer offers: GIF and WebP join only for a daemon
 * that agreed `attachments.gif_webp`. */
export function acceptedImageTypes(gifWebpSupported: boolean): readonly AcceptedMimeType[] {
  return gifWebpSupported ? WITH_GIF_WEBP_MIME_TYPES : BASE_MIME_TYPES;
}

/** One pick, split by where each file goes. */
export interface PickedRoute {
  /** Read now and carried inline on the send. */
  images: readonly PromptAttachment[];
  /** Handed to the file route, their bytes untouched until it chunks them. */
  files: readonly File[];
}

/**
 * Split one pick — a multi-select or a drop — between the image route and the
 * file route.
 *
 * The image route takes a file whose MIME type is accepted, whose size the
 * deposit can carry (128 KiB), and for which there is still `room` left; every
 * other file is a file attachment, and its bytes are not read here at all.
 */
export async function routePickedFiles(
  picked: readonly File[],
  acceptedTypes: readonly AcceptedMimeType[],
  room: number,
): Promise<PickedRoute> {
  const images: PromptAttachment[] = [];
  const files: File[] = [];
  let remaining = Math.max(0, room);
  for (const file of picked) {
    const mimeType = acceptedTypes.find((candidate) => candidate === file.type);
    if (mimeType === undefined || file.size > MAX_CHAT_IMAGE_BYTES || remaining <= 0) {
      files.push(file);
      continue;
    }
    remaining -= 1;
    const bytes = new Uint8Array(await file.arrayBuffer());
    images.push({ name: file.name, mimeType, data: base64Of(bytes) });
  }
  return { images, files };
}

function previewUrl(attachment: PromptAttachment): string {
  return `data:${attachment.mimeType};base64,${attachment.data}`;
}

/**
 * The composer's attach control: the button, the hidden multi-select input,
 * the image previews with their remove buttons, the attached-file chips, and
 * the refusal sentence.
 *
 * One pick is partitioned here: an image the composer can carry inline is read
 * and handed to `onAdd` as a prompt attachment, and everything else — another
 * type, an image past the 128 KiB the deposit accepts, an image with no room
 * left — is handed to `onAddFiles` as the raw `File`, whose bytes the file
 * route reads in chunks later. An image pick therefore never costs the memory
 * of a file the composer will not carry inline.
 */
export function ComposerAttachControl({
  images,
  files,
  disabled,
  sending,
  overflowNotice,
  gifWebpSupported,
  onAdd,
  onRemove,
  onAddFiles,
  onRemoveFile,
}: {
  images: readonly PromptAttachment[];
  files: readonly AttachedFile[];
  disabled: boolean;
  /** A send is in flight: picks and removals wait for its answer. */
  sending: boolean;
  /** A handed-back row's images that did not fit beside current picks. */
  overflowNotice: string | null;
  /** The daemon agreed `attachments.gif_webp`: GIF and WebP join the pickable types. */
  gifWebpSupported: boolean;
  onAdd: (images: readonly PromptAttachment[]) => void;
  onRemove: (index: number) => void;
  /** Absent for a composer that carries no file attachments: the file route
   * files are then dropped rather than read. */
  onAddFiles?: (files: readonly File[]) => void;
  onRemoveFile: (id: string) => void;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const pickerDisabled = disabled || sending;
  const acceptedTypes = acceptedImageTypes(gifWebpSupported);

  async function handleFiles(picked: FileList | null) {
    if (picked === null) return;
    const room = Math.max(0, MAX_COMPOSER_IMAGES - images.length);
    const route = await routePickedFiles(Array.from(picked), acceptedTypes, room);
    if (route.images.length > 0) onAdd(route.images);
    if (route.files.length > 0) onAddFiles?.(route.files);
    if (inputRef.current !== null) inputRef.current.value = "";
  }

  return (
    <div className={`workspace-composer-images${sending ? " is-sending" : ""}`}>
      <ComposerFileChips files={files} disabled={pickerDisabled} onRemove={onRemoveFile} />
      {images.length > 0 ? (
        <div className="workspace-composer-previews">
          {images.map((attachment, position) => (
            <div
              key={`${attachment.name}-${position}`}
              className="workspace-composer-preview"
              data-testid="composer-image-preview"
            >
              <img src={previewUrl(attachment)} alt={`Attached image preview ${position + 1}`} />
              <button
                type="button"
                className="workspace-composer-preview-remove"
                aria-label={`Remove attached image ${position + 1}`}
                onClick={() => onRemove(position)}
                disabled={pickerDisabled}
              >
                ×
              </button>
            </div>
          ))}
        </div>
      ) : null}
      {overflowNotice !== null ? (
        <div className="workspace-composer-image-refusal" role="alert">
          {overflowNotice}
        </div>
      ) : null}
      <button
        type="button"
        className="workspace-composer-attach"
        aria-label="Attach file"
        title="Attach file"
        onClick={() => inputRef.current?.click()}
        disabled={pickerDisabled}
      >
        <svg
          width={14}
          height={14}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth={1.75}
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
          focusable="false"
        >
          <rect x={3} y={3} width={18} height={18} rx={2} />
          <circle cx={9} cy={9} r={2} />
          <path d="m21 15-5-5L5 21" />
        </svg>
      </button>
      <input
        ref={inputRef}
        type="file"
        className="workspace-composer-image-input"
        data-testid="composer-image-input"
        multiple
        disabled={pickerDisabled}
        onChange={(event) => void handleFiles(event.currentTarget.files)}
      />
    </div>
  );
}
