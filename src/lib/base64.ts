/**
 * Base64 of one byte array, without blowing the argument list: one 32 KiB
 * chunk at a time through `String.fromCharCode`.
 *
 * The whole array is encoded in one string; the chunking is only how the code
 * units get into `btoa`, which is why the result has no padding of its own —
 * `btoa` supplies it.
 */
export function base64Of(bytes: Uint8Array): string {
  let text = "";
  const STEP = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += STEP) {
    text += String.fromCharCode(...bytes.subarray(offset, offset + STEP));
  }
  return btoa(text);
}
