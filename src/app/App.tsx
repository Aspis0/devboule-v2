import { lazy, Suspense } from 'react';
import type { ComponentType } from 'react';
import { Shell } from './Shell';
import { SurfacePlaceholder } from './SurfacePlaceholder';
import { useAppStore } from '../store/appStore';
import { SURFACES, type SurfaceDefinition, type SurfaceKey } from '../types/surface';

interface SurfaceRendererProps {
  surface: SurfaceDefinition;
}

type SurfaceComponent = ComponentType<SurfaceRendererProps>;

const LazyWorkspace = lazy(() =>
  import('../features/workspace/Workspace').then(({ Workspace }) => ({
    default: (_props: SurfaceRendererProps) => <Workspace />,
  })),
);

const LazySettings = lazy(() =>
  import('../features/settings/SettingsSurface').then(({ SettingsSurface }) => ({
    default: (_props: SurfaceRendererProps) => <SettingsSurface />,
  })),
);

const LazyDesign = lazy(() =>
  import('../features/design/DesignSurface').then(({ DesignSurface }) => ({
    default: (_props: SurfaceRendererProps) => <DesignSurface />,
  })),
);

const SURFACE_COMPONENTS: Record<SurfaceKey, SurfaceComponent> = {
  workspace: LazyWorkspace,
  polis: SurfacePlaceholder,
  oracle: SurfacePlaceholder,
  pubvia: SurfacePlaceholder,
  design: LazyDesign,
  settings: LazySettings,
};

function SurfaceLoading() {
  return <div className="surface-loading" role="status">Loading…</div>;
}

export function App() {
  const activeSurface = useAppStore((state) => state.activeSurface);
  const surface = SURFACES.find((item) => item.key === activeSurface) ?? SURFACES[0];
  const SurfaceComponent = SURFACE_COMPONENTS[surface.key];

  return (
    <Shell activeSurface={surface.key}>
      <Suspense fallback={<SurfaceLoading />}>
        <SurfaceComponent surface={surface} />
      </Suspense>
    </Shell>
  );
}
