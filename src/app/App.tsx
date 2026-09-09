import { lazy, Suspense } from "react";
import type { ComponentType } from "react";
import { Shell } from "./Shell";
import { SurfacePlaceholder } from "./SurfacePlaceholder";
import { useAppStore } from "../store/appStore";
import { SURFACES, type SurfaceDefinition, type SurfaceKey } from "../types/surface";

interface SurfaceRendererProps {
  surface: SurfaceDefinition;
}

type SurfaceComponent = ComponentType<SurfaceRendererProps>;

const LazyWorkspace = lazy(() =>
  import("../features/workspace/Workspace").then(({ Workspace }) => ({
    default: () => <Workspace />,
  })),
);

const LazySettings = lazy(() =>
  import("../features/settings/SettingsSurface").then(({ SettingsSurface }) => ({
    default: () => <SettingsSurface />,
  })),
);

const LazyPolis = lazy(() =>
  import("../features/polis/PolisSurface").then(({ PolisSurface }) => ({
    default: PolisSurface,
  })),
);

const LazyDesign = lazy(() =>
  import("../features/design/DesignSurface").then(({ DesignSurface }) => ({
    default: () => <DesignSurface />,
  })),
);

const LazyMarketplace = lazy(() =>
  import("../features/marketplace/MarketplaceSurface").then(({ MarketplaceSurface }) => ({
    default: () => <MarketplaceSurface />,
  })),
);

const SURFACE_COMPONENTS: Record<SurfaceKey, SurfaceComponent> = {
  workspace: LazyWorkspace,
  polis: LazyPolis,
  pubvia: SurfacePlaceholder,
  design: LazyDesign,
  settings: LazySettings,
  marketplace: LazyMarketplace,
};

function SurfaceLoading() {
  return (
    <div className="surface-loading" role="status">
      Loading…
    </div>
  );
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
