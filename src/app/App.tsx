import { lazy, Suspense, useEffect, useState } from "react";
import type { ComponentType } from "react";
import { Shell } from "./Shell";
import { SurfacePlaceholder } from "./SurfacePlaceholder";
import { oracleStatus } from "../lib/tauri";
import { useAppStore, type DesignSessionState } from "../store/appStore";
import type { OracleIndexStatus } from "../types/ipc";
import { SURFACES, type SurfaceDefinition, type SurfaceKey } from "../types/surface";
import type {
  DesignHost,
  DesignSurfaceProps,
} from "../features/design/DesignSurface";
import { createAgentHost, disposeAgentHost } from "../features/design/agentHost";
import { createDemoHost } from "../features/design/mockData";
import { createOracleHost } from "../features/design/oracleHost";

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

const DEMO_DESIGN_HOST = createDemoHost();
const ORACLE_DESIGN_HOST = createOracleHost();

type DesignHostKind = "agent" | "oracle" | "demo";

let resolvedDesignHostKind: DesignHostKind | null = null;
let designHostResolution: Promise<DesignHostKind> | null = null;
let designBoundaryLifecycle = 0;

function oracleCanAnswer(status: OracleIndexStatus): boolean {
  return (
    status.state !== "error" &&
    status.state !== "indexing" &&
    status.indexed_files > 0 &&
    status.model.state === "ready" &&
    status.reranker?.state !== "downloading" &&
    status.reranker?.state !== "missing"
  );
}

interface DesignHostBoundaryProps {
  DesignSurface: ComponentType<DesignSurfaceProps>;
}

function resolveDesignHostKind(): Promise<DesignHostKind> {
  if (resolvedDesignHostKind !== null) return Promise.resolve(resolvedDesignHostKind);
  if (designHostResolution !== null) return designHostResolution;

  const pending = Promise.allSettled([oracleStatus()]).then(([oracleResult]) => {
    const kind: DesignHostKind =
      oracleResult.status === "fulfilled" && oracleCanAnswer(oracleResult.value) ? "agent" : "demo";
    resolvedDesignHostKind = kind;
    return kind;
  });
  designHostResolution = pending;
  void pending.finally(() => {
    if (designHostResolution === pending) designHostResolution = null;
  });
  return pending;
}

function selectedDesignHost(kind: DesignHostKind): DesignHost {
  const currentHost = useAppStore.getState().designSession.host;
  if (currentHost !== null) return currentHost;

  const host =
    kind === "agent"
      ? createAgentHost()
      : kind === "oracle"
        ? ORACLE_DESIGN_HOST
        : DEMO_DESIGN_HOST;
  useAppStore.getState().setDesignHost(host);
  return host;
}

function designHasWork(session: DesignSessionState): boolean {
  // There is no discard/new-document action yet, so a message or artifact keeps
  // this session live for the application's lifetime once it has been created.
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
    let active = true;

    void resolveDesignHostKind().then((kind) => {
      if (!active) return;
      const host = selectedDesignHost(kind);
      setSelection(host);
    });

    return () => {
      active = false;
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
