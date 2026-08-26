import { useState } from 'react';
import type { PointerEvent, ReactNode } from 'react';
import { useAppStore } from '../store/appStore';
import { SURFACES, type SurfaceKey } from '../types/surface';

interface ShellProps {
  activeSurface: SurfaceKey;
  children: ReactNode;
}

const NAV_POINTS = [
  { key: 'workspace', x: 260, y: 39 },
  { key: 'polis', x: 344, y: 68 },
  { key: 'oracle', x: 430, y: 87 },
  { key: 'pubvia', x: 516, y: 87 },
  { key: 'design', x: 602, y: 68 },
  { key: 'settings', x: 686, y: 39 },
] as const satisfies readonly { key: SurfaceKey; x: number; y: number }[];

export function Shell({ activeSurface, children }: ShellProps) {
  const selectSurface = useAppStore((state) => state.selectSurface);
  const [navOpen, setNavOpen] = useState(false);
  const pageShift = navOpen ? '42px' : '0px';
  const pageDim = navOpen ? 0.78 : 1;

  function handlePointerMove(event: PointerEvent<HTMLElement>) {
    const y = event.clientY;
    if (y <= 110) {
      setNavOpen(true);
    } else if (navOpen && y > 150) {
      setNavOpen(false);
    }
  }

  return (
    <main
      className="app-shell"
      onPointerMove={handlePointerMove}
      onPointerLeave={() => setNavOpen(false)}
    >
      <div
        className="page-layer"
        style={{ transform: `translateY(${pageShift})`, opacity: pageDim }}
      >
        {children}
      </div>

      <div className="crescent-shell" aria-label="Devboule surfaces">
        <button
          type="button"
          className="crescent-sliver"
          aria-label="Reveal navigation"
          aria-expanded={navOpen}
          onMouseEnter={() => setNavOpen(true)}
        />

        <div
          className={`crescent-nav${navOpen ? ' crescent-nav-open' : ''}`}
          onMouseEnter={() => setNavOpen(true)}
          onMouseLeave={(event) => {
            if (event.clientY > 150) {
              setNavOpen(false);
            }
          }}
        >
          <div className="crescent-glow" aria-hidden="true" />
          <svg className="crescent-arc" viewBox="0 0 880 150" aria-hidden="true">
            <path d="M 222 21.9 A 410 410 0 0 0 724 21.9" />
            <path className="crescent-arc-border" d="M 222 21.9 A 410 410 0 0 0 724 21.9" />
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
                  setNavOpen(false);
                }}
                tabIndex={navOpen ? 0 : -1}
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
