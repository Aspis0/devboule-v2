import { stat } from "node:fs/promises";

export async function selectFreshestBackend(candidates) {
  const present = [];
  for (const path of candidates) {
    try {
      const metadata = await stat(path);
      if (metadata.isFile()) {
        present.push({ path, mtimeMs: metadata.mtimeMs });
      }
    } catch (error) {
      if (error?.code !== "ENOENT") {
        throw error;
      }
    }
  }

  if (present.length === 0) {
    return null;
  }
  const newest = Math.max(...present.map(({ mtimeMs }) => mtimeMs));
  const matches = present.filter(({ mtimeMs }) => mtimeMs === newest);
  if (matches.length > 1) {
    throw new Error(
      `ambiguous backend candidates have the same mtime: ${matches
        .map(({ path }) => path)
        .join(", ")}`,
    );
  }
  return matches[0].path;
}
