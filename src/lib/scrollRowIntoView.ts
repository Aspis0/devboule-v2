/** Scrolls a menu row visible inside its own box: a bare scrollIntoView would
 * walk every scrollable ancestor — transcript and page included. A row below
 * the window lands at its bottom minus the window height, not minus the
 * window bottom, which already contains the current scroll. */
export function scrollRowIntoView(list: HTMLElement, row: HTMLElement): void {
  const rowTop = row.offsetTop;
  const rowBottom = rowTop + row.offsetHeight;
  const viewTop = list.scrollTop;
  const viewBottom = viewTop + list.clientHeight;
  if (rowTop < viewTop) list.scrollTop = rowTop;
  else if (rowBottom > viewBottom) list.scrollTop = rowBottom - list.clientHeight;
}
