import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent, ReactNode } from "react";
import { chooseAndInstall } from "../features/plugins/install";
import { pluginState } from "../lib/plugins";
import { useAppStore } from "../store/appStore";
import { SURFACES, type SurfaceDefinition, type SurfaceKey } from "../types/surface";

interface ShellProps {
  activeSurface: SurfaceKey;
  children: ReactNode;
}

const NAV_POINTS = [
  { key: "workspace", x: 277, y: 44 },
  { key: "polis", x: 371, y: 80 },
  { key: "pubvia", x: 470, y: 92 },
  { key: "design", x: 569, y: 80 },
  { key: "settings", x: 662, y: 44 },
] as const satisfies readonly { key: SurfaceKey; x: number; y: number }[];

/**
 * What a point in the crescent is offering.
 *
 * Only a surface that comes from a plugin has more than one: the rest are
 * always `open`. `unknown` is deliberately distinct from `add` — before the
 * inventory has arrived we have not looked, and drawing a `+` then would tell
 * the user something is missing when nobody has checked.
 */
type PointOffer = "open" | "add" | "installing" | "broken" | "unknown";

function offerFor(
  surface: SurfaceDefinition,
  plugins: ReturnType<typeof useAppStore.getState>["plugins"],
  installing: string | null,
): PointOffer {
  const pluginId = surface.plugin;
  if (!pluginId) return "open";
  if (installing === pluginId) return "installing";
  if (!plugins) return "unknown";
  switch (pluginState(plugins, pluginId).kind) {
    case "ready":
      return "open";
    case "absent":
      return "add";
    case "refused":
    case "unknown":
      return "broken";
  }
}

const GLYPH: Record<Exclude<PointOffer, "open" | "installing">, string> = {
  add: "+",
  broken: "!",
  unknown: "·",
};

export function Shell({ activeSurface, children }: ShellProps) {
  const selectSurface = useAppStore((state) => state.selectSurface);
  const plugins = useAppStore((state) => state.plugins);
  const installing = useAppStore((state) => state.installing);
  const refreshPlugins = useAppStore((state) => state.refreshPlugins);
  const [navOpen, setNavOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const suppressTriggerFocusRef = useRef(false);
  const pageShift = navOpen ? "34px" : "0px";
  const pageDim = navOpen ? 0.34 : 1;

  // Asked once, on the way in: the crescent has to know whether Polis is
  // something to open or something to add before it is first drawn.
  useEffect(() => {
    void refreshPlugins();
  }, [refreshPlugins]);

  function closeNav() {
    setNavOpen(false);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      closeNav();
      suppressTriggerFocusRef.current = true;
      triggerRef.current?.focus();
    }
  }

  // Pointer movement only ever CLOSES the crescent. Opening is reserved to the
  // 13px sliver and to keyboard focus: an open-on-move band across the top of
  // the window would shift and dim the page while the pointer is still aiming
  // at the surface toolbar underneath it.
  function handlePointerMove(event: PointerEvent<HTMLElement>) {
    if (navOpen && event.clientY > 150) {
      closeNav();
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
          onFocus={() => {
            if (suppressTriggerFocusRef.current) {
              suppressTriggerFocusRef.current = false;
              return;
            }
            setNavOpen(true);
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " " || event.key === "ArrowDown") {
              event.preventDefault();
              setNavOpen(true);
            }
          }}
        />

        <div
          id="devboule-crescent-navigation"
          className={`crescent-nav${navOpen ? " crescent-nav-open" : ""}`}
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
            const offer = offerFor(surface, plugins, installing);
            const pluginId = "plugin" in surface ? surface.plugin : undefined;
            return (
              <button
                type="button"
                key={surface.key}
                className={`nav-point nav-point-${offer}${isActive ? " nav-point-active" : ""}`}
                style={{ left: point.x, top: point.y }}
                aria-label={
                  offer === "open"
                    ? `Open ${surface.label}`
                    : offer === "add"
                      ? `Install ${surface.label}`
                      : offer === "installing"
                        ? `Installing ${surface.label}`
                        : offer === "broken"
                          ? `${surface.label} unavailable`
                          : `${surface.label} status unknown`
                }
                aria-busy={offer === "installing"}
                disabled={offer === "installing"}
                onClick={() => {
                  // The `+` adds the plugin; every other state opens the
                  // surface, including the broken one — that is where the
                  // reason it was refused is written out.
                  if (offer === "add" && pluginId) {
                    void chooseAndInstall(pluginId, surface.label);
                    return;
                  }
                  selectSurface(surface.key);
                  closeNav();
                  triggerRef.current?.focus();
                }}
                onFocus={() => setNavOpen(true)}
                tabIndex={0}
                aria-current={isActive ? "page" : undefined}
              >
                <span className="nav-point-circle">
                  {offer === "installing" ? (
                    <span className="nav-point-spinner" aria-hidden="true" />
                  ) : (
                    <span aria-hidden="true">
                      {offer === "open" ? surface.label.slice(0, 1) : GLYPH[offer]}
                    </span>
                  )}
                </span>
                <span className="nav-point-label">{surface.label}</span>
              </button>
            );
          })}
        </div>

        <svg
          className={`crescent-hint${navOpen ? " crescent-hint-hidden" : ""}`}
          viewBox="0 0 880 40"
          aria-hidden="true"
        >
          <path d="M 340 14 A 410 410 0 0 0 600 14" />
        </svg>
      </div>
    </main>
  );
}
