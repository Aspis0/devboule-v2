import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent, ReactNode } from "react";
import { chooseAndInstall } from "../features/plugins/install";
import { pluginState } from "../lib/plugins";
import { useAppStore } from "../store/appStore";
import { SURFACES, type SurfaceDefinition, type SurfaceKey } from "../types/surface";
import { CRESCENT_LABEL_MAX_WIDTH, CRESCENT_VISIBLE_COUNT, layoutCrescent } from "./crescentLayout";

interface ShellProps {
  activeSurface: SurfaceKey;
  children: ReactNode;
}

const SURFACE_KEYS = SURFACES.map((surface) => surface.key) satisfies SurfaceKey[];

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
  const installError = useAppStore((state) => state.installError);
  const dismissInstallError = useAppStore((state) => state.dismissInstallError);
  const refreshPlugins = useAppStore((state) => state.refreshPlugins);
  const [navOpen, setNavOpen] = useState(false);
  const [surfaceOffset, setSurfaceOffset] = useState(0);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const navigationRef = useRef<HTMLDivElement>(null);
  const suppressTriggerFocusRef = useRef(false);
  const pendingFocusRef = useRef<{
    surfaceKey?: string;
    delta: -1 | 1;
    focusedArrow: boolean;
  } | null>(null);
  const navOpenRef = useRef(navOpen);
  const visibleKeysRef = useRef<string[]>([]);
  const pageShift = navOpen ? "34px" : "0px";
  const pageDim = navOpen ? 0.34 : 1;
  const crescentLayout = layoutCrescent(SURFACE_KEYS, CRESCENT_VISIBLE_COUNT, surfaceOffset);
  navOpenRef.current = navOpen;
  visibleKeysRef.current = crescentLayout.visibleKeys;

  // Asked once, on the way in: the crescent has to know whether Polis is
  // something to open or something to add before it is first drawn.
  useEffect(() => {
    void refreshPlugins();
  }, [refreshPlugins]);

  function closeNav() {
    setNavOpen(false);
    setSurfaceOffset(0);
  }

  function pageBy(delta: -1 | 1) {
    const focusedElement = document.activeElement;
    pendingFocusRef.current = {
      surfaceKey:
        focusedElement instanceof HTMLElement ? focusedElement.dataset.surfaceKey : undefined,
      delta,
      focusedArrow:
        focusedElement instanceof HTMLElement &&
        focusedElement.classList.contains("crescent-page-arrow") &&
        focusedElement.dataset.surfaceKey === undefined,
    };
    setSurfaceOffset((offset) => offset + delta);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLElement>) {
    if (event.key === "Escape") {
      if (!navOpen) return;
      event.preventDefault();
      closeNav();
      if (document.activeElement === triggerRef.current) {
        suppressTriggerFocusRef.current = false;
      } else {
        suppressTriggerFocusRef.current = true;
        triggerRef.current?.focus();
      }
      return;
    }
    if (!navOpen) return;
    if (
      event.target instanceof Element &&
      event.target.closest("input, textarea, select, [contenteditable]") !== null
    ) {
      return;
    }
    if (event.key === "ArrowLeft" && crescentLayout.canPrev) {
      event.preventDefault();
      pageBy(-1);
    } else if (event.key === "ArrowRight" && crescentLayout.canNext) {
      event.preventDefault();
      pageBy(1);
    }
  }

  useLayoutEffect(() => {
    const pendingFocus = pendingFocusRef.current;
    pendingFocusRef.current = null;
    if (!navOpenRef.current) return;
    if (pendingFocus === null) return;

    const visibleKeys = visibleKeysRef.current;

    const keepsFocusedKey =
      pendingFocus.surfaceKey !== undefined &&
      visibleKeys.some((key) => key === pendingFocus.surfaceKey);
    if (!keepsFocusedKey && pendingFocus.focusedArrow) {
      const arrowLabel = pendingFocus.delta === 1 ? "Show next surfaces" : "Show previous surfaces";
      const matchingArrow = navigationRef.current?.querySelector<HTMLButtonElement>(
        `[aria-label="${arrowLabel}"]`,
      );
      if (matchingArrow !== null && matchingArrow !== undefined) {
        matchingArrow.focus();
        return;
      }
    }

    const focusKey = keepsFocusedKey
      ? pendingFocus.surfaceKey
      : pendingFocus.delta === 1
        ? visibleKeys.at(-1)
        : visibleKeys[0];
    if (focusKey === undefined) return;

    const nextFocus = Array.from(
      navigationRef.current?.querySelectorAll<HTMLButtonElement>(".nav-point") ?? [],
    ).find((button) => button.dataset.surfaceKey === focusKey);
    nextFocus?.focus();
  }, [surfaceOffset]);

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
          ref={navigationRef}
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

          {navOpen && installError ? (
            <div className="crescent-install-error" role="alert">
              <span>The last install did not happen — {installError}</span>
              <button type="button" onClick={dismissInstallError}>
                Dismiss
              </button>
            </div>
          ) : null}

          {navOpen && crescentLayout.canPrev ? (
            <button
              type="button"
              className="crescent-page-arrow crescent-page-arrow-prev"
              aria-label="Show previous surfaces"
              onClick={() => pageBy(-1)}
            >
              ‹
            </button>
          ) : null}
          {navOpen && crescentLayout.canNext ? (
            <button
              type="button"
              className="crescent-page-arrow crescent-page-arrow-next"
              aria-label="Show next surfaces"
              onClick={() => pageBy(1)}
            >
              ›
            </button>
          ) : null}

          {crescentLayout.points.map((point) => {
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
                data-surface-key={surface.key}
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
                <span className="nav-point-label" style={{ maxWidth: CRESCENT_LABEL_MAX_WIDTH }}>
                  {surface.label}
                </span>
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
