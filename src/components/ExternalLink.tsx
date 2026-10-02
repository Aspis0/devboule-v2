import type { MouseEvent as ReactMouseEvent, ReactNode } from "react";
import { openInBrowser, opensExternally } from "../lib/openInBrowser";

/**
 * A link whose activation opens the system browser instead of the webview: a
 * primary click, keyboard Enter and the middle button all route through the
 * command. Only a URL the command would open has its click cancelled, so every
 * other destination keeps the browser's own behaviour.
 */
export function ExternalLink({ href, children }: { href: string; children: ReactNode }) {
  const opens = (event: ReactMouseEvent<HTMLAnchorElement>) => {
    if (!opensExternally(href)) return;
    event.preventDefault();
    openInBrowser(href);
  };
  return (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      onClick={opens}
      onAuxClick={(event) => {
        if (event.button === 1) opens(event);
      }}
    >
      {children}
    </a>
  );
}
