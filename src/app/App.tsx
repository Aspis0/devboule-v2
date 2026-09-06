import { lazy, Suspense, useEffect, useState } from "react";
import type { ComponentType } from "react";
import { Shell } from "./Shell";
import { SurfacePlaceholder } from "./SurfacePlaceholder";
import { oracleStatus } from "../lib/tauri";
import { useAppStore } from "../store/appStore";
import type { OracleIndexStatus } from "../types/ipc";
import { SURFACES, type SurfaceDefinition, type SurfaceKey } from "../types/surface";
import type { DesignHost, DesignSurfaceProps } from "../features/design/DesignSurface";
import {
  createAgentHost,
  disposeAgentHost,
  resolveAgentWorkspace,
} from "../features/design/agentHost";
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
const DEMO_DESIGN_DISCLOSURE = "Demo design — fixtures, not a live store.";
const ORACLE_DESIGN_DISCLOSURE = "Repository index — Oracle results, no design writes.";
const AGENT_DESIGN_DISCLOSURE = "Repository agent — ACP writes in the first workspace it finds.";

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

function DesignHostBoundary({ DesignSurface }: DesignHostBoundaryProps) {
  const [selection, setSelection] = useState<{ host: DesignHost; disclosure: string } | null>(null);

  useEffect(() => {
    const agentHost = createAgentHost();
    let active = true;

    void Promise.allSettled([oracleStatus(), resolveAgentWorkspace()]).then(
      ([oracleResult, workspaceResult]) => {
        if (!active) return;
        if (
          oracleResult.status === "fulfilled" &&
          oracleCanAnswer(oracleResult.value) &&
          workspaceResult.status === "fulfilled"
        ) {
          setSelection({ host: agentHost, disclosure: AGENT_DESIGN_DISCLOSURE });
        } else if (oracleResult.status === "fulfilled" && oracleCanAnswer(oracleResult.value)) {
          setSelection({ host: ORACLE_DESIGN_HOST, disclosure: ORACLE_DESIGN_DISCLOSURE });
        } else {
          setSelection({ host: DEMO_DESIGN_HOST, disclosure: DEMO_DESIGN_DISCLOSURE });
        }
      },
    );

    return () => {
      active = false;
      void disposeAgentHost(agentHost);
    };
  }, []);

  if (selection === null) return <SurfaceLoading />;

  return <DesignSurface host={selection.host} disclosure={selection.disclosure} />;
}

const LazyDesign = lazy(() =>
  import("../features/design/DesignSurface").then(({ DesignSurface }) => ({
    default: () => <DesignHostBoundary DesignSurface={DesignSurface} />,
  })),
);

const SURFACE_COMPONENTS: Record<SurfaceKey, SurfaceComponent> = {
  workspace: LazyWorkspace,
  polis: LazyPolis,
  pubvia: SurfacePlaceholder,
  design: LazyDesign,
  settings: LazySettings,
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
