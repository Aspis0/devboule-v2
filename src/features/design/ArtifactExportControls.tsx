import { ArtifactCopyControl } from "./ArtifactCopyControl";
import { ArtifactPrintControl } from "./ArtifactPrintControl";
import { ArtifactSaveControl } from "./ArtifactSaveControl";
import type { DesignOutputMode } from "./designHost";

interface ArtifactExportControlsProps {
  /** Current artifact markup. Required: the surface renders nothing when no artifact is on screen. */
  html: string;
  /** Title of the assistant message that produced the artifact, for the exported document. */
  title?: string;
  /** How the artifact paginates when printed; supplied by the surface, never defaulted here. */
  outputMode?: DesignOutputMode;
}

export function ArtifactExportControls({ html, title, outputMode }: ArtifactExportControlsProps) {
  return (
    <>
      <ArtifactCopyControl html={html} title={title} />
      <ArtifactSaveControl html={html} title={title} />
      <ArtifactPrintControl html={html} title={title} outputMode={outputMode} />
    </>
  );
}
