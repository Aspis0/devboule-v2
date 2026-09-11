import { lazy, Suspense, useEffect, useState } from "react";
import type { ComponentType } from "react";
import { Shell } from "./Shell";
import { SurfacePlaceholder } from "./SurfacePlaceholder";
import { useAppStore, type DesignSessionState } from "../store/appStore";
import { SURFACES, type SurfaceDefinition, type SurfaceKey } from "../types/surface";
import type { DesignHost, DesignSurfaceProps } from "../features/design/DesignSurface";
import { createAgentHost, disposeAgentHost } from "../features/design/agentHost";

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

let designBoundaryLifecycle = 0;

interface DesignHostBoundaryProps {
  DesignSurface: ComponentType<DesignSurfaceProps>;
}

// The Design surface always runs on the agent host. Generation grounds on
// the attached folder's own index (or runs ungrounded with no folder), so
// the global Oracle index state never decides which host the user gets.
function selectedDesignHost(): DesignHost {
  const currentHost = useAppStore.getState().designSession.host;
  if (currentHost !== null) return currentHost;

  const host = createAgentHost();
  useAppStore.getState().setDesignHost(host);
  return host;
}

function designHasWork(session: DesignSessionState): boolean {
  // A worked Design host intentionally survives surface navigation so its transcript, artifact,
  // and live agent context are available when the user returns. The visible Design "End session"
  // control is the user-directed teardown; an unworked host is disposed on navigation.
  return (
    session.generation !== null || session.latestArtifact !== null || session.messages.length > 0
  );
}

async function releaseDesignHostIfUnused(): Promise<void> {
  const session = useAppStore.getState().designSession;
  if (designHasWork(session) || session.host === null) return;

  const host = session.host;
  useAppStore.getState().clearDesignSession(host);
  await disposeAgentHost(host);
}

function DesignHostBoundary({ DesignSurface }: DesignHostBoundaryProps) {
  const [selection, setSelection] = useState<DesignHost | null>(null);

  useEffect(() => {
    const lifecycle = ++designBoundaryLifecycle;
    setSelection(selectedDesignHost());

    return () => {
      queueMicrotask(() => {
        if (designBoundaryLifecycle === lifecycle) void releaseDesignHostIfUnused();
      });
    };
  }, []);

  if (selection === null) return <SurfaceLoading />;

  return <DesignSurface host={selection} />;
}

const LazyDesign = lazy(() =>
  import("../features/design/DesignSurface").then(({ DesignSurface }) => ({
    default: () => <DesignHostBoundary DesignSurface={DesignSurface} />,
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
