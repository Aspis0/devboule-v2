/**
 * The host's future CKG will provide this same shape over the bridge. The
 * checked-in JSON is only a build-time stand-in so the renderer can be tested
 * before the host has a route for live city data.
 */
export interface City {
  files: CityFile[];
  imports: CityImport[];
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
