import { Shell } from './Shell';
import { SurfacePlaceholder } from './SurfacePlaceholder';
import { Workspace } from '../features/workspace/Workspace';
import { useAppStore } from '../store/appStore';
import { SURFACES } from '../types/surface';

export function App() {
  const activeSurface = useAppStore((state) => state.activeSurface);
  const surface = SURFACES.find((item) => item.key === activeSurface) ?? SURFACES[0];

  return (
    <Shell activeSurface={surface.key}>
      {activeSurface === 'workspace' ? <Workspace /> : <SurfacePlaceholder surface={surface} />}
    </Shell>
  );
}
