import { useRef, useState } from 'react';
import type { KeyboardEvent, PointerEvent, ReactNode } from 'react';
import { useAppStore } from '../store/appStore';
import { SURFACES, type SurfaceKey } from '../types/surface';

interface ShellProps {
  activeSurface: SurfaceKey;
  children: ReactNode;
}

const NAV_POINTS = [
  { key: 'workspace', x: 277, y: 44 },
  { key: 'polis', x: 371, y: 80 },
  { key: 'pubvia', x: 470, y: 92 },
  { key: 'design', x: 569, y: 80 },
  { key: 'settings', x: 662, y: 44 },
] as const satisfies readonly { key: SurfaceKey; x: number; y: number }[];

export function Shell({ activeSurface, children }: ShellProps) {
  const selectSurface = useAppStore((state) => state.selectSurface);
  const [navOpen, setNavOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const pageShift = navOpen ? '34px' : '0px';
  const pageDim = navOpen ? 0.34 : 1;

  function closeNav() {
    setNavOpen(false);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLElement>) {
    if (event.key === 'Escape') {
      event.preventDefault();
      closeNav();
      triggerRef.current?.focus();
    }
  }

  // Pointer movement only ever CLOSES the crescent. Opening is reserved to the
  // 13px sliver and to keyboard focus: an open-on-move band across the top of
  // the window would shift and dim the page while the pointer is still aiming
  // at the surface toolbar underneath it.
  function handlePointerMove(event: PointerEvent<HTMLElement>) {
    if (navOpen && event.clientY > 150) {
      setNavOpen(false);
    }
  }

  return (
    <main
      className="app-shell"
      onPointerMove={handlePointerMove}
      onPointerLeave={closeNav}
      onKeyDown={handleKeyDown}
    >
      <div
        className="page-layer"
        style={{ transform: `translateY(${pageShift})`, opacity: pageDim }}
      >
        {children}
      </div>

      <div className="crescent-shell" role="navigation" aria-label="Devboule surfaces">
        <button
          type="button"
          ref={triggerRef}
          className="crescent-sliver"
          aria-label="Reveal navigation"
          aria-expanded={navOpen}
          aria-controls="devboule-crescent-navigation"
          onPointerEnter={() => setNavOpen(true)}
          onFocus={() => setNavOpen(true)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' || event.key === ' ' || event.key === 'ArrowDown') {
              event.preventDefault();
              setNavOpen(true);
            }
          }}
        />

        <div
          id="devboule-crescent-navigation"
          className={`crescent-nav${navOpen ? ' crescent-nav-open' : ''}`}
          onPointerEnter={() => setNavOpen(true)}
          onPointerLeave={(event) => {
            if (event.clientY > 150) {
              closeNav();
            }
          }}
        >
          <div className="crescent-glow" aria-hidden="true" />
          <svg className="crescent-arc" viewBox="0 0 880 150" aria-hidden="true">
            <path d="M 240.8 21.9 A 410 410 0 0 0 699.2 21.9" />
            <path className="crescent-arc-border" d="M 240.8 21.9 A 410 410 0 0 0 699.2 21.9" />
          </svg>

          {NAV_POINTS.map((point) => {
            const surface = SURFACES.find((item) => item.key === point.key);
            if (!surface) return null;
            const isActive = surface.key === activeSurface;
            return (
              <button
                type="button"
                key={surface.key}
                className={`nav-point${isActive ? ' nav-point-active' : ''}`}
                style={{ left: point.x, top: point.y }}
                onClick={() => {
                  selectSurface(surface.key);
                  closeNav();
                  triggerRef.current?.focus();
                }}
                onFocus={() => setNavOpen(true)}
                tabIndex={0}
                aria-current={isActive ? 'page' : undefined}
              >
                <span className="nav-point-circle">
                  <span aria-hidden="true">{surface.label.slice(0, 1)}</span>
                </span>
                <span className="nav-point-label">{surface.label}</span>
              </button>
            );
          })}
        </div>

        <svg className={`crescent-hint${navOpen ? ' crescent-hint-hidden' : ''}`} viewBox="0 0 880 40" aria-hidden="true">
          <path d="M 340 14 A 410 410 0 0 0 600 14" />
        </svg>
      </div>
    </main>
  );
}
