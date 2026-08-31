/**
 * The host's future CKG will provide this same shape over the bridge. The
 * checked-in JSON is only a build-time stand-in so the renderer can be tested
 * before the host has a route for live city data.
 */
export interface City {
  files: CityFile[];
  imports: CityImport[];
  /** These overlays are fixture data until the host routes the live feeds. */
  agents: CityAgent[];
  findings: CityFinding[];
  dataSource?: "fixture" | "host";
}

export interface CityFile {
  id: string;
  path: string;
  lines: number;
  district: string;
}

export interface CityImport {
  from: string;
  to: string;
  weight: number;
}

export type CityAgentState = "working" | "silent" | "finished" | "idle";

/** A provider-backed CLI session observed by the host. */
export interface CityAgent {
  id: string;
  provider: string;
  state: CityAgentState;
  /** Null means the session belongs in the roster, not at an invented map point. */
  fileId: string | null;
}

export type CityFindingSeverity = "smoke" | "fire" | "inferno";

/**
 * The renderer-facing finding contract. `fileId` is the resolved building key;
 * the host's Finding.file path is mapped to it at the bridge boundary. The
 * overlapping fields deliberately keep the mapping shallow and visible.
 */
export interface CityFinding {
  id: string;
  fileId: string;
  severity: CityFindingSeverity;
  rule: string;
  title: string;
}
