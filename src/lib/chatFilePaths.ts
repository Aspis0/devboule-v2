// The root-relative resolution follows Paseo's packages/app/src/assistant-file-links/parse.ts resolveRelativePathUnderRoot (Copyright (c) 2025-present Mohamed Boudra, Apache-2.0). Modified by Devboule.

export interface ChatFileLink {
  /** Workspace-relative path with `/` separators — the form the daemon's
   * file APIs and the tab identities use. */
  relativePath: string;
  /** The same file resolved against the session cwd to an absolute
   * spelling — the opener's form, so a link in a subdirectory session
   * opens the file it names instead of a root-relative lookalike. Null
   * for `~` paths (no home to resolve against here; the app road
   * expands those) and when no root is known and the spelling is
   * relative. */
  absolutePath: string | null;
  /** 1-based line and column from a `:line` / `:line:col` suffix. Carried
   * for a later slice; the current tab opener does not scroll. */
  line?: number;
  column?: number;
}

/** The narrow context a workspace hands the transcript: where agent paths
 * resolve, and what opening one does. Null turns recognition off. */
export interface ChatFileLinks {
  root: string;
  open: (relativePath: string) => void;
}

/** A recognized token inside one plain-text segment, at raw offsets. */
export interface ChatFilePathToken {
  start: number;
  end: number;
  link: ChatFileLink;
}

// Bound candidate parsing independently of the message size.
const MAX_TOKEN_LENGTH = 1024;
const MAX_LINE_DIGITS = 9;
const LINE_SUFFIX = new RegExp(
  `^(.+?):(\\d{1,${MAX_LINE_DIGITS}})(?::(\\d{1,${MAX_LINE_DIGITS}}))?$`,
);
// Accept a filename ending in a dot plus 1–10 ASCII alphanumeric extension
// characters, including dot-leading names such as `.env` and `..ts`.
const FILE_EXTENSION = /^.*\.[A-Za-z0-9]{1,10}$/;
const NUMERIC_FILENAME = /^[\d.]+$/;
const EXTENSIONLESS_FILENAMES = new Set([
  "LICENSE",
  "VERSION",
  ".gitattributes",
  ".oracleignore",
  "LICENSE.refero_skill",
  "NOTICE",
  "RECORD",
  "WHEEL",
  "Makefile",
  "Dockerfile",
]);
// Sentence punctuation and closing quotes/brackets lean on a path without
// being part of it: `src/x.ts.` ends a sentence, `src/x.ts)` a parenthesis.
const TRAILING_PUNCTUATION = ".,;:!?)]}\"'";
// Keep fragments in the run so a fragment-bearing path is rejected whole.
const PATH_CHARACTER = /[\p{L}\p{N}._\-/\\:+#~]/u;

type WorkspaceRoot = { drive: string | null; segments: string[] };

/** Parses one candidate token against the workspace root. Null leaves the
 * candidate as plain text: outside the root, escaping it, a URL, a bare
 * name, a directory, or anything malformed. */
export function parseChatFilePath(candidate: string, root: string): ChatFileLink | null {
  return parseCandidate(candidate, normalizeRoot(root), isUncRoot(root));
}

export function parseChatCodeFilePath(candidate: string, root: string): ChatFileLink | null {
  return parseCandidate(candidate, normalizeRoot(root), isUncRoot(root), true);
}

/** A root this parser cannot represent: a UNC share (`\\server\…`).
 * Kept refusing even for absolute spellings — without a drive to
 * compare, an absolute spelling proves nothing. An absent root (no cwd
 * known) is different: relative paths stay plain, absolute ones link in
 * their own spelling. */
function isUncRoot(root: string): boolean {
  return root.replace(/\\/g, "/").startsWith("//");
}

function parseCandidate(
  candidate: string,
  root: WorkspaceRoot | null,
  uncRoot = false,
  codeSpan = false,
): ChatFileLink | null {
  const stripped = codeSpan ? candidate : stripTrailingPunctuation(candidate);
  if (stripped.length === 0 || stripped.length > MAX_TOKEN_LENGTH) return null;
  let path = stripped;
  let line: number | undefined;
  let column: number | undefined;
  const suffix = LINE_SUFFIX.exec(stripped);
  if (suffix !== null) {
    path = suffix[1];
    line = Number(suffix[2]);
    column = suffix[3] === undefined ? undefined : Number(suffix[3]);
    // A zero line or column is not a location an editor could show; treat
    // the whole token as text rather than open the path without it.
    if (line === 0 || column === 0) return null;
  }
  const invalidCharacters = codeSpan ? /[^\S ]|[\p{Cc}#]/u : /[\s\p{Cc}#]/u;
  if (path.length === 0 || invalidCharacters.test(path)) return null;
  if (codeSpan && path !== path.trim()) return null;
  // A `~` anywhere but the lead is not a home folder: the daemon expands
  // a leading `~/` only, so anything else stays plain text.
  if (path.includes("~") && path !== "~" && !path.startsWith("~/")) return null;
  // A bare name needs a by-name lookup this slice does not have.
  if (!path.includes("/") && !path.includes("\\")) return null;
  const link = resolveAgainstRoot(path, root, uncRoot, line, column);
  if (link !== null && codeSpan && !matchesDisplayedPath(path, root, link.relativePath))
    return null;
  return link;
}

// Refuse component normalization so the code label cannot silently name a different file.
function matchesDisplayedPath(
  path: string,
  root: WorkspaceRoot | null,
  relativePath: string,
): boolean {
  if (path === relativePath) return true;
  const absolutePath = normalizeSeparators(path).replace(
    /^([A-Za-z]):/,
    (_, drive: string) => `${drive.toUpperCase()}:`,
  );
  // An outside absolute link travels in its own spelling: the displayed
  // path is the link when it normalises to the same spelling.
  if (/^(?:[A-Za-z]:)?\//.test(relativePath)) return absolutePath === relativePath;
  if (root === null) return false;
  const prefix = root.drive === null ? "/" : `${root.drive}:/`;
  return absolutePath === prefix + [...root.segments, relativePath].join("/");
}

/** Finds every recognized path token in one plain-text segment. Runs are
 * maximal, so a URL is rejected whole instead of leaving a tail behind. */
export function scanChatFilePaths(text: string, root: string): ChatFilePathToken[] {
  const base = normalizeRoot(root);
  const unc = isUncRoot(root);
  const tokens: ChatFilePathToken[] = [];
  let start = -1;
  let spacedAbsolute = false;
  for (let index = 0; index <= text.length; index += 1) {
    const verbatimMarker =
      text[index] === "?" &&
      index === start + 2 &&
      text[start] === "\\" &&
      text[start + 1] === "\\";
    const onPath = index < text.length && (PATH_CHARACTER.test(text[index]) || verbatimMarker);
    if (onPath && start < 0) start = index;
    if (onPath || start < 0) continue;
    if (index - start <= MAX_TOKEN_LENGTH) {
      const candidate = stripTrailingPunctuation(text.slice(start, index));
      const link = hasMarkdownEscape(text, start, index)
        ? null
        : parseCandidate(candidate, base, unc);
      if (link !== null && !spacedAbsolute)
        tokens.push({ start, end: start + candidate.length, link });
      const path = splitPath(candidate);
      const filename = path?.segments.at(-1) ?? "";
      // A spaced absolute path must not leave a relative-looking tail behind.
      spacedAbsolute =
        text[index] === " " && (spacedAbsolute || path?.absolute === true) && !isFilename(filename);
    } else {
      spacedAbsolute = false;
    }
    start = -1;
  }
  return tokens;
}

function isFilename(filename: string): boolean {
  return (
    !NUMERIC_FILENAME.test(filename) &&
    (FILE_EXTENSION.test(filename) || EXTENSIONLESS_FILENAMES.has(filename))
  );
}

// Prose decodes escaped punctuation, so these bytes cannot safely name a file.
function hasMarkdownEscape(text: string, start: number, end: number): boolean {
  for (let index = start; index < end; index += 1) {
    if (text[index] === "\\" && isMarkdownPunctuation(text.charCodeAt(index + 1))) return true;
  }
  return false;
}

function isMarkdownPunctuation(code: number): boolean {
  return (
    (code >= 33 && code <= 47) ||
    (code >= 58 && code <= 64) ||
    (code >= 91 && code <= 96) ||
    (code >= 123 && code <= 126)
  );
}

function stripTrailingPunctuation(candidate: string): string {
  let end = candidate.length;
  while (end > 0 && TRAILING_PUNCTUATION.includes(candidate[end - 1])) end -= 1;
  return candidate.slice(0, end);
}

function normalizeSeparators(path: string): string {
  return path.replace(/^\\\\\?\\(?=[A-Za-z]:[\\/])/, "").replace(/\\/g, "/");
}

/** A candidate ending in a separator is a directory; workspace roots may
 * keep that separator. Unexplained colons reject schemes and drive-relative paths. */
function splitPath(
  path: string,
  allowTrailingSeparator = false,
): { drive: string | null; absolute: boolean; segments: string[] } | null {
  const normalized = normalizeSeparators(path);
  if (normalized.startsWith("//")) return null;
  const drive = /^([A-Za-z]):\//.exec(normalized);
  let rest = normalized;
  let driveLetter: string | null = null;
  let absolute = false;
  if (drive !== null) {
    driveLetter = drive[1].toUpperCase();
    rest = normalized.slice(2);
    absolute = true;
  } else {
    if (normalized.includes(":")) return null;
    absolute = normalized.startsWith("/");
  }
  if (rest.endsWith("/") && !allowTrailingSeparator) return null;
  return { drive: driveLetter, absolute, segments: rest.split("/") };
}

function normalizeRoot(root: string): WorkspaceRoot | null {
  const base = splitPath(root, true);
  if (base === null || !base.absolute) return null;
  const segments = resolveSegments(base.segments);
  return segments === null ? null : { drive: base.drive, segments };
}

// Compare root components, not prefixes: a sibling like repo2 is outside repo.
function absoluteSpellingOf(candidate: { drive: string | null }, resolved: string[]): string {
  return candidate.drive === null
    ? `/${resolved.join("/")}`
    : `${candidate.drive}:/${resolved.join("/")}`;
}

function joinAbsolute(root: WorkspaceRoot, resolved: string[]): string {
  const prefix = root.drive === null ? "/" : `${root.drive}:/`;
  return prefix + [...root.segments, ...resolved].join("/");
}

function resolveAgainstRoot(
  path: string,
  root: WorkspaceRoot | null,
  uncRoot: boolean,
  line: number | undefined,
  column: number | undefined,
): ChatFileLink | null {
  const candidate = splitPath(path);
  if (candidate === null) return null;
  const resolved = resolveSegments(candidate.segments);
  if (resolved === null || resolved.length === 0) return null;
  const filename = resolved[resolved.length - 1];
  if (!isFilename(filename)) return null;
  const relativePath = resolved.join("/");
  // The opener's form: the file resolved against the session cwd to an
  // absolute spelling, so a subdirectory session's link cannot open (or
  // create, on save) a root-relative lookalike. Null when there is
  // nothing to resolve against (`~` travels on its own spelling; a
  // relative path with no known root stays relative-only).
  const absolutePath =
    candidate.segments[0] === "~"
      ? null
      : candidate.absolute
        ? absoluteSpellingOf(candidate, resolved)
        : root === null
          ? null
          : joinAbsolute(root, resolved);
  if (!candidate.absolute) {
    // A leading `~/` names the human's home, never the workspace: it
    // travels in its own spelling and the app road expands it.
    if (candidate.segments[0] === "~") {
      return { relativePath: `~/${resolved.slice(1).join("/")}`, absolutePath, line, column };
    }
    return { relativePath, absolutePath, line, column };
  }
  // An absolute path outside the workspace stays clickable in its own
  // spelling: the File tab routes it app-only to this machine's own
  // file, never joined to the root. Inside the root it resolves
  // relatively, exactly as before.
  const absoluteSpelling = absoluteSpellingOf(candidate, resolved);
  // A root this parser cannot represent (a UNC share) keeps refusing:
  // without a drive to compare, an absolute spelling proves nothing.
  // With no root at all, relative paths stay plain but an absolute
  // spelling links in its own spelling — the opener routes it (remote
  // key or app road), never joined to anything.
  if (root === null) {
    if (uncRoot) return null;
    if (!candidate.absolute) {
      if (candidate.segments[0] !== "~") return null;
      return { relativePath: `~/${resolved.slice(1).join("/")}`, absolutePath, line, column };
    }
    return { relativePath: absoluteSpelling, absolutePath, line, column };
  }
  if (root.drive !== candidate.drive)
    return { relativePath: absoluteSpelling, absolutePath, line, column };
  const rootSegments = root.segments;
  if (resolved.length <= rootSegments.length) return null;
  for (let index = 0; index < rootSegments.length; index += 1) {
    const candidateSegment = resolved[index];
    const rootSegment = rootSegments[index];
    if (
      root.drive === null
        ? candidateSegment !== rootSegment
        : foldAsciiCase(candidateSegment) !== foldAsciiCase(rootSegment)
    )
      return { relativePath: absoluteSpelling, absolutePath, line, column };
  }
  return {
    relativePath: resolved.slice(rootSegments.length).join("/"),
    // An absolute spelling resolves to itself; a relative one resolves
    // against the root it was parsed under.
    absolutePath: candidate.absolute ? absoluteSpelling : joinAbsolute(root, resolved),
    line,
    column,
  };
}

function foldAsciiCase(value: string): string {
  return value.replace(/[A-Z]/g, (character) => character.toLowerCase());
}

/** Collapses `.`, empty and duplicate separators; null when a `..` escapes
 * the top. An empty result names the root itself, which is never a file. */
function resolveSegments(segments: string[]): string[] | null {
  const resolved: string[] = [];
  for (const segment of segments) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      if (resolved.length === 0) return null;
      resolved.pop();
      continue;
    }
    // Refuse dot/space-only names across platforms; Windows trims them to empty.
    if (segment.replace(/[ .]+$/, "") === "") return null;
    resolved.push(segment);
  }
  return resolved;
}
