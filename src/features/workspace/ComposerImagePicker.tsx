import { useRef, useState } from "react";
import type { PromptAttachment } from "../../types/ipc";

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

/** Base64 without blowing the argument list: one 32 KiB chunk at a time. */
function base64Of(bytes: Uint8Array): string {
  let text = "";
  const STEP = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += STEP) {
    text += String.fromCharCode(...bytes.subarray(offset, offset + STEP));
  }
  return btoa(text);
}

function previewUrl(attachment: PromptAttachment): string {
  return `data:${attachment.mimeType};base64,${attachment.data}`;
}

/**
 * The composer's image attach control: the button, the file input, the
 * previews with their remove buttons, and the refusal sentences. It
 * validates picks and reads their bytes; the picked list itself lives in
 * the composer, which sends it.
 */
export function ComposerImagePicker({
  images,
  disabled,
  sending,
  overflowNotice,
  gifWebpSupported,
  onAdd,
  onRemove,
}: {
  images: readonly PromptAttachment[];
  disabled: boolean;
  /** A send is in flight: picks and removals wait for its answer. */
  sending: boolean;
  /** A handed-back row's images that did not fit beside current picks. */
  overflowNotice: string | null;
  /** The daemon agreed `attachments.gif_webp`: GIF and WebP join the pickable types. */
  gifWebpSupported: boolean;
  onAdd: (images: readonly PromptAttachment[]) => void;
  onRemove: (index: number) => void;
}) {
  const inputRef = useRef<HTMLInputElement>(null);
  const [pickRefusal, setPickRefusal] = useState<string | null>(null);
  const pickerDisabled = disabled || sending;
  const refusal = overflowNotice ?? pickRefusal;
  const acceptedTypes: readonly AcceptedMimeType[] = gifWebpSupported
    ? WITH_GIF_WEBP_MIME_TYPES
    : BASE_MIME_TYPES;

  async function handleFiles(files: FileList | null) {
    if (files === null) return;
    const accepted: PromptAttachment[] = [];
    let refused: string | null = null;
    for (const file of Array.from(files)) {
      if (images.length + accepted.length >= MAX_COMPOSER_IMAGES) {
        refused = `The composer carries at most ${MAX_COMPOSER_IMAGES} images.`;
        break;
      }
      const mimeType = acceptedTypes.find((accepted) => accepted === file.type);
      if (mimeType === undefined) {
        refused = `${file.name} is not an image the composer can attach.`;
        continue;
      }
      if (file.size > MAX_CHAT_IMAGE_BYTES) {
        refused = `${file.name} is larger than 128 KiB.`;
        continue;
      }
      const bytes = new Uint8Array(await file.arrayBuffer());
      accepted.push({ name: file.name, mimeType, data: base64Of(bytes) });
    }
    setPickRefusal(refused);
    if (accepted.length > 0) onAdd(accepted);
    if (inputRef.current !== null) inputRef.current.value = "";
  }

  return (
    <div className={`workspace-composer-images${sending ? " is-sending" : ""}`}>
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
      {refusal !== null ? (
        <div className="workspace-composer-image-refusal" role="alert">
          {refusal}
        </div>
      ) : null}
      <button
        type="button"
        className="workspace-composer-attach"
        aria-label="Attach image"
        title="Attach image"
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
        accept={acceptedTypes.join(",")}
        multiple
        disabled={pickerDisabled}
        onChange={(event) => void handleFiles(event.currentTarget.files)}
      />
    </div>
  );
}
