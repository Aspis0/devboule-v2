import type { ReactElement } from "react";

export interface ErrorTextProps {
  sentence: string;
  detail: string | null;
  /**
   * Names the hidden detail node; every render site passes its own, so two
   * mounted surfaces never share one id.
   */
  id: string;
}

/**
 * A mapped sentence whose raw detail stays present: `title` carries it for
 * the mouse; `aria-describedby` points at a visually hidden node that
 * screen readers read in browse mode. Neither channel is keyboard-focusable,
 * so a keyboard-only user gets the sentence but not the detail.
 */
export function ErrorText({ sentence, detail, id }: ErrorTextProps): ReactElement {
  const detailId = `${id}-detail`;
  return (
    <>
      <span title={detail ?? undefined} aria-describedby={detail !== null ? detailId : undefined}>
        {sentence}
      </span>
      {detail !== null ? (
        <span id={detailId} className="error-detail-sr-only">
          {detail}
        </span>
      ) : null}
    </>
  );
}
