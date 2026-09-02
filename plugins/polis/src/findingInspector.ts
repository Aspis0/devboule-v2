import type { CityFile, CityFinding, CityFindingSeverity } from "./model";
import {
  INSPECT_FETCH_TIMEOUT_MS,
  ORACLE_SEARCH_TIMEOUT_MS,
  loadFindingInspection,
  loadOracleCitations,
  type FindingInspectionFailure,
  type FindingInspectionLoadState,
  type HostInvoker,
  type OracleCitation,
  type OracleIndex,
  type OracleCitationsFailure,
  type OracleCitationsLoadState,
} from "./hostBridge";

const SEVERITY_RANK: Record<CityFindingSeverity, number> = {
  smoke: 1,
  fire: 2,
  inferno: 3,
};

const SEVERITY_COLORS: Record<CityFindingSeverity, string> = {
  smoke: "#9daab3",
  fire: "#f08a3d",
  inferno: "#e84c3d",
};

export interface FindingInspector {
  open(file: CityFile, findings: readonly CityFinding[]): void;
  refreshFindings(findings: readonly CityFinding[]): void;
  close(): void;
  destroy(): void;
  isOpen(): boolean;
}

export interface FindingInspectorNavigation {
  /** The city file for a citation path, or null when no building matches. */
  resolveFile: (path: string) => CityFile | null;
  /** Re-open the inspector on that building; main.ts supplies its findings. */
  openFile: (file: CityFile) => void;
}

/** Keep the inspector's building lookup identical to the renderer's current findings input. */
export function indexFindingsByFile(
  index: Map<string, CityFinding[]>,
  findings: readonly CityFinding[],
): void {
  index.clear();
  for (const finding of findings) {
    const fileFindings = index.get(finding.fileId);
    if (fileFindings === undefined) index.set(finding.fileId, [finding]);
    else fileFindings.push(finding);
  }
}

export function createFindingInspector(
  container: HTMLElement,
  invoke: HostInvoker,
  onClose: (file: CityFile) => void = () => undefined,
  navigation?: FindingInspectorNavigation,
): FindingInspector {
  let activeFile: CityFile | null = null;
  let activeFindings: CityFinding[] = [];
  let selectedFindingId: string | null = null;
  let requestGeneration = 0;
  let citationGeneration = 0;
  let cachedOracleCitations: OracleCitationsLoadState | null = null;
  let pageHidden = false;

  container.classList.add("polis-finding-inspector");
  container.style.maxHeight = "min(42vh, 24rem)";
  container.style.overflowY = "auto";

  const onKeyDown = (event: KeyboardEvent): void => {
    if (event.key === "Escape" && activeFile !== null) close();
  };
  const onPageHide = (): void => {
    pageHidden = true;
    requestGeneration += 1;
    invalidateCitations();
    document.removeEventListener("keydown", onKeyDown);
    window.removeEventListener("pagehide", onPageHide);
  };
  document.addEventListener("keydown", onKeyDown);
  window.addEventListener("pagehide", onPageHide, { once: true });

  function open(file: CityFile, findings: readonly CityFinding[]): void {
    activeFile = file;
    activeFindings = findingsForFile(file.id, findings);
    selectedFindingId = null;
    requestGeneration += 1;
    invalidateCitations();
    renderPanel();
    const token = citationGeneration;
    if (!pageHidden) {
      void loadOracleCitationsForFile(file.path, token).catch(() => undefined);
    }
  }

  function refreshFindings(findings: readonly CityFinding[]): void {
    if (activeFile === null) return;
    requestGeneration += 1;
    selectedFindingId = null;
    activeFindings = findingsForFile(activeFile.id, findings);
    renderPanel();
  }

  function close(): void {
    if (activeFile === null) return;
    const file = activeFile;
    activeFile = null;
    activeFindings = [];
    selectedFindingId = null;
    requestGeneration += 1;
    invalidateCitations();
    container.replaceChildren();
    onClose(file);
  }

  function destroy(): void {
    requestGeneration += 1;
    invalidateCitations();
    activeFile = null;
    activeFindings = [];
    selectedFindingId = null;
    document.removeEventListener("keydown", onKeyDown);
    window.removeEventListener("pagehide", onPageHide);
    container.replaceChildren();
  }

  function isOpen(): boolean {
    return activeFile !== null;
  }

  function invalidateCitations(): void {
    citationGeneration += 1;
    cachedOracleCitations = null;
  }

  function renderPanel(): void {
    const file = activeFile;
    if (file === null) return;
    container.replaceChildren();

    const header = document.createElement("div");
    header.className = "polis-inspector-header";
    const heading = document.createElement("strong");
    heading.textContent = "Building inspector";
    const closeButton = document.createElement("button");
    closeButton.type = "button";
    closeButton.className = "polis-inspector-close";
    closeButton.setAttribute("aria-label", "Close building inspector");
    closeButton.textContent = "Close";
    closeButton.addEventListener("click", close);
    header.append(heading, closeButton);
    container.appendChild(header);

    const fileReadout = document.createElement("div");
    fileReadout.className = "polis-inspector-file";
    fileReadout.textContent = `${file.path} · ${file.lines.toLocaleString()} lines · district ${file.district}`;
    container.appendChild(fileReadout);

    const list = document.createElement("div");
    list.className = "polis-finding-list";
    const findings = [...activeFindings].sort(compareFindings);
    if (findings.length === 0) {
      const empty = document.createElement("div");
      empty.className = "polis-finding-empty";
      empty.textContent = "No findings for this building.";
      list.appendChild(empty);
    } else {
      for (const finding of findings) {
        const row = document.createElement("button");
        row.type = "button";
        row.className = "polis-finding-row";
        row.dataset.findingId = finding.id;
        const chip = document.createElement("span");
        chip.className = "polis-finding-severity";
        chip.textContent = finding.severity;
        chip.style.backgroundColor = SEVERITY_COLORS[finding.severity];
        chip.style.color = finding.severity === "smoke" ? "#07101c" : "#ffffff";
        const title = document.createElement("span");
        title.className = "polis-finding-title";
        title.textContent = finding.title;
        const rule = document.createElement("span");
        rule.className = "polis-finding-rule";
        rule.textContent = finding.rule;
        row.append(chip, title, rule);
        row.addEventListener("click", () => {
          void inspectFinding(finding).catch(() => undefined);
        });
        list.appendChild(row);
      }
    }
    container.appendChild(list);

    const detail = document.createElement("div");
    detail.className = "polis-finding-detail";
    detail.textContent =
      findings.length === 0
        ? "Select a finding to inspect its lines."
        : "Select a finding for line details.";
    container.appendChild(detail);

    const oracle = document.createElement("section");
    oracle.className = "polis-oracle-citations";
    const oracleHeading = document.createElement("strong");
    oracleHeading.textContent = "Oracle pointers";
    const oracleSubtitle = document.createElement("div");
    oracleSubtitle.textContent =
      "Ranked by similarity to this file's path. Oracle points, it does not answer.";
    const oracleBody = document.createElement("div");
    oracleBody.className = "polis-oracle-citations-body";
    if (cachedOracleCitations === null) {
      oracleBody.textContent = "Oracle pointers: searching…";
    } else {
      renderOracleCitations(oracleBody, cachedOracleCitations, navigation, file.path);
    }
    oracle.append(oracleHeading, oracleSubtitle, oracleBody);
    container.appendChild(oracle);
  }

  async function loadOracleCitationsForFile(query: string, token: number): Promise<void> {
    const state = await loadOracleCitations(invoke, query, ORACLE_SEARCH_TIMEOUT_MS);
    if (pageHidden || token !== citationGeneration || activeFile === null) return;
    cachedOracleCitations = state;
    const target = container.querySelector<HTMLElement>(".polis-oracle-citations-body");
    if (target === null) return;
    renderOracleCitations(target, state, navigation, activeFile.path);
  }

  async function inspectFinding(finding: CityFinding): Promise<void> {
    const token = ++requestGeneration;
    selectedFindingId = finding.id;
    const detail = container.querySelector<HTMLElement>(".polis-finding-detail");
    if (detail === null) return;
    detail.textContent = "Finding details: loading…";
    const state = await loadFindingInspection(invoke, finding.id, INSPECT_FETCH_TIMEOUT_MS);
    if (
      pageHidden ||
      token !== requestGeneration ||
      activeFile === null ||
      selectedFindingId !== finding.id
    ) {
      return;
    }
    renderInspectionState(detail, state);
  }

  return { open, refreshFindings, close, destroy, isOpen };
}

export function renderInspectionState(
  container: HTMLElement,
  state: FindingInspectionLoadState,
): void {
  container.replaceChildren();
  if (state.status === "failed") {
    container.textContent = inspectionFailureCopy(state.failure);
    return;
  }

  const lines = document.createElement("div");
  lines.className = "polis-finding-lines";
  const spans = state.inspection.locations.map((location) =>
    formatLineSpan(location.startLine, location.endLine),
  );
  const spanReadout = spans.length > 1 ? ` · spans: ${spans.join(", ")}` : "";
  lines.textContent = `Lines: ${formatLineSpan(state.inspection.startLine, state.inspection.endLine)}${spanReadout}`;
  const detector = document.createElement("div");
  detector.className = "polis-finding-detector";
  detector.textContent = `Detector: ${state.inspection.source}`;
  container.append(lines, detector);

  if (state.inspection.source.trim().toLowerCase() === "secrets") {
    const evidence = document.createElement("div");
    evidence.className = "polis-finding-evidence-note";
    evidence.textContent =
      "Evidence is withheld for secret findings; secret values never leave the scanner.";
    container.appendChild(evidence);
  }
}

function renderOracleCitations(
  container: HTMLElement,
  state: OracleCitationsLoadState,
  navigation: FindingInspectorNavigation | undefined,
  activePath: string,
): void {
  container.replaceChildren();
  if (state.status === "failed") {
    container.textContent = oracleCitationsFailureCopy(state.failure);
    return;
  }

  if (state.citations.results.length === 0) {
    container.textContent = oracleEmptyStateCopy(state.citations.index);
    return;
  }

  const list = document.createElement("div");
  list.className = "polis-oracle-citation-list";
  state.citations.results.forEach((citation, index) => {
    const file = navigation?.resolveFile(citation.path) ?? null;
    if (navigation === undefined || file === null) {
      const row = document.createElement("div");
      row.className = "polis-oracle-citation-plain";
      row.textContent = formatOracleCitation(index, citation, citation.path === activePath);
      list.appendChild(row);
      return;
    }

    const row = document.createElement("button");
    row.className = "polis-oracle-citation-button";
    row.type = "button";
    row.addEventListener("click", () => navigation.openFile(file));
    row.textContent = formatOracleCitation(index, citation, citation.path === activePath);
    list.appendChild(row);
  });
  container.appendChild(list);
}

function formatOracleCitation(
  index: number,
  citation: OracleCitation,
  isCurrentFile: boolean,
): string {
  const lineReadout =
    citation.startLine === 0
      ? "lines unknown"
      : formatLineSpan(citation.startLine, citation.endLine);
  const parts = [`#${String(index + 1).padStart(2, "0")}`, citation.path, lineReadout];
  if (isCurrentFile) parts.push("this file");
  if (citation.focusStartLine !== undefined && citation.focusEndLine !== undefined) {
    parts.push(`start at ${formatLineRange(citation.focusStartLine, citation.focusEndLine)}`);
  }
  if (citation.symbol !== undefined) parts.push(`symbol ${citation.symbol}`);
  if (citation.match !== undefined) parts.push(`match ${citation.match}`);
  return parts.join(" · ");
}

function formatLineRange(startLine: number, endLine: number): string {
  return startLine === endLine ? String(startLine) : `${startLine}–${endLine}`;
}

function oracleEmptyStateCopy(index: OracleIndex | undefined): string {
  if (index?.state === "indexing") {
    return "Oracle is still indexing this workspace.";
  }
  if (index?.state === "error") {
    return "Oracle's index is in an error state.";
  }
  if (index?.indexedFiles === 0) {
    return "No Oracle index for this workspace yet — build it in Settings › Oracle.";
  }
  return "No spans matched this file.";
}

function oracleCitationsFailureCopy(failure: OracleCitationsFailure): string {
  switch (failure) {
    case "timeout":
      return "Oracle pointers unavailable: the search timed out.";
    case "busy":
      return "Oracle pointers unavailable: another search is still running.";
    case "invalid":
      return "Oracle pointers unavailable: the host rejected the query.";
    case "refusal":
      return "Oracle pointers unavailable: the host refused the request.";
    case "malformed":
      return "Oracle pointers unavailable: the host returned an invalid response.";
  }
}

function findingsForFile(fileId: string, findings: readonly CityFinding[]): CityFinding[] {
  return findings.filter((finding) => finding.fileId === fileId);
}

function compareFindings(left: CityFinding, right: CityFinding): number {
  return (
    SEVERITY_RANK[right.severity] - SEVERITY_RANK[left.severity] ||
    (left.id < right.id ? -1 : left.id > right.id ? 1 : 0)
  );
}

function formatLineSpan(startLine: number, endLine: number): string {
  return startLine === endLine ? `line ${startLine}` : `lines ${startLine}–${endLine}`;
}

function inspectionFailureCopy(failure: FindingInspectionFailure): string {
  switch (failure) {
    case "timeout":
      return "Finding details timed out.";
    case "refusal":
      return "Finding details were refused by the backend.";
    case "malformed":
      return "Finding details were malformed.";
    case "not_found":
      return "This finding expired; the scanner no longer has it.";
  }
}
