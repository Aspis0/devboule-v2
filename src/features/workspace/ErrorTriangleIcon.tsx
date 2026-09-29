/** The spec's error triangle. The class is fixed because it carries the
 * 12px box and the currentColor fill; the mark adds no style of its own. */
export function ErrorTriangleIcon() {
  return (
    <svg
      className="workspace-error-line-icon"
      viewBox="0 0 12 12"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M6 1.6 11 10.4H1Z" />
    </svg>
  );
}
