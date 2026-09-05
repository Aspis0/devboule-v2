import { oracleAsk } from "../../lib/tauri";
import type { OracleSearchResponse } from "../../types/ipc";
import type { DesignDocument, DesignGenerationResult, DesignHost } from "./designHost";
import { createDemoHost } from "./mockData";

function throwIfAborted(signal: AbortSignal): void {
  if (signal.aborted) {
    throw new DOMException("Generation aborted", "AbortError");
  }
}

function describeResults(response: OracleSearchResponse): string {
  const count = response.results.length;
  if (count === 0) {
    return `Nothing found in the repository index for "${response.query}".`;
  }

  const noun = count === 1 ? "hit" : "hits";
  return `Oracle found ${count} ${noun} in the repository index for "${response.query}".`;
}

export function createOracleHost(): DesignHost {
  const documentHost = createDemoHost();

  return {
    loadDocument: async (): Promise<DesignDocument> => {
      const document = await documentHost.loadDocument();
      return { ...document, messages: [] };
    },
    generate: async (prompt, signal): Promise<DesignGenerationResult> => {
      throwIfAborted(signal);
      const response = await oracleAsk(prompt);
      throwIfAborted(signal);

      return {
        prompt,
        title:
          response.results.length === 0
            ? "Nothing found in Oracle"
            : `Oracle result${response.results.length === 1 ? "" : "s"}`,
        desc: describeResults(response),
        sources: response.results.map((result) => result.path),
        nodeIds: [],
      };
    },
  };
}
