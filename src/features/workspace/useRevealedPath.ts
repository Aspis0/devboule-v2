import { useEffect, useRef } from "react";

/** A request to show one file in the Changes panel. A new nonce is a new request. */
export interface RevealRequest {
  path: string;
  nonce: number;
}

/**
 * Selects the requested file once per nonce. A request already applied is not
 * replayed when `select` changes identity, so the person's own choice after it
 * stands until the next request.
 */
export function useRevealedPath(
  reveal: RevealRequest | null | undefined,
  select: (path: string) => void,
): void {
  const applied = useRef<number | null>(null);
  const nonce = reveal?.nonce ?? null;
  const path = reveal?.path ?? null;
  useEffect(() => {
    if (nonce === null || path === null || applied.current === nonce) return;
    applied.current = nonce;
    select(path);
  }, [nonce, path, select]);
}
