import type { SurfaceDefinition } from "../types/surface";

interface SurfacePlaceholderProps {
  surface: SurfaceDefinition;
}

export function SurfacePlaceholder({ surface }: SurfacePlaceholderProps) {
  return (
    <section className="surface-card" aria-labelledby={`${surface.key}-title`}>
      <header className="surface-header">
        <div className="surface-header-title">
          <span className={`surface-dot surface-dot-${surface.tone}`} aria-hidden="true" />
          <h1>{surface.label}</h1>
          <span className="surface-divider" aria-hidden="true" />
          <span className="surface-eyebrow">{surface.eyebrow}</span>
        </div>
        <span className="surface-status">M1a shell</span>
      </header>

      <div className="surface-body">
        <div className={`placeholder-panel placeholder-panel-${surface.tone}`}>
          <span className="placeholder-kicker">Surface placeholder</span>
          <div className="placeholder-mark" aria-hidden="true">
            {surface.label.slice(0, 1)}
          </div>
          <h2 id={`${surface.key}-title`}>{surface.label} surface</h2>
          <p>{surface.description}</p>
          <span className="placeholder-note">
            Navigation is live · content lands in its milestone
          </span>
        </div>
      </div>
    </section>
  );
}
