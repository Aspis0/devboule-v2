/**
 * The quiet retry: one icon button with the label, shared by the branch row,
 * the state screens and the agent-edits head. Loading, a refused first read and
 * a folder that is not a repository all keep it: a refused read is exactly when
 * a manual retry matters, and the poll is no substitute for it.
 */
export function RefreshButton({
  onRefresh,
  iconSize = 12,
}: {
  onRefresh: () => void;
  iconSize?: number;
}) {
  return (
    <button
      type="button"
      className="workspace-changes-refresh"
      aria-label="Refresh"
      title="Refresh"
      onClick={onRefresh}
    >
      <svg width={iconSize} height={iconSize} viewBox="0 0 24 24" fill="none" aria-hidden="true">
        <path
          d="M21 12a9 9 0 1 1-2.64-6.36"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
        />
        <path
          d="M21 3v6h-6"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    </button>
  );
}
