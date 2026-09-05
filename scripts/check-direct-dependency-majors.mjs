import { execFile } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const MAX_ATTEMPTS = 3;
const REQUEST_TIMEOUT_MS = 15_000;
const RETRY_DELAY_MS = 1_000;
const CRATES_IO_SOURCE = "registry+https://github.com/rust-lang/crates.io-index";

/*
 * Exceptions are intentionally inline in the checker, not in a separate
 * config file. If a deliberate major-version lag becomes necessary, add it
 * in the relevant map with both fields below:
 *
 *   'package-name': {
 *     reason: 'why the major-version lag is deliberate',
 *     exitCondition: 'what must become true before removing the exception',
 *   }
 *
 * An exception can suppress a major-version finding only. It cannot suppress
 * a malformed declaration or a registry/network failure.
 */
const inlineExceptions = {
  npm: {},
  cargo: {
    reqwest: {
      reason:
        "0.12.28 is what the workspace already resolves via oracle-core (hf-hub, lancedb), so the daemon reuses that artifact; 0.13's rustls feature drags in the aws-lc-rs native cmake/NASM build, which the CI runner cannot be assumed to carry",
      exitCondition:
        "oracle-core's dependents (hf-hub, lance-namespace-reqwest-client) move to reqwest 0.13, or reqwest regains a ring-based TLS feature",
    },
  },
};

const npmRegistryUrl = (name) => `https://registry.npmjs.org/${encodeURIComponent(name)}/latest`;

const cargoRegistryUrl = (name) => `https://crates.io/api/v1/crates/${encodeURIComponent(name)}`;

const sleep = (milliseconds) =>
  new Promise((resolvePromise) => setTimeout(resolvePromise, milliseconds));

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error);
}

// The unit of breaking change is not always the leading number. Under semver,
// and under the caret/tilde rules that both Cargo and npm apply, a 0.x release
// treats the MINOR as the major: 0.32 and 0.40 are incompatible, and 0.0.x
// treats the PATCH that way. Reading only the leading integer reports every
// 0.x dependency as current forever, which is precisely where staleness hides
// in the Rust ecosystem. Returns a single comparable key, not a major number.
function parseMajor(versionLike) {
  if (typeof versionLike !== "string") return null;

  const match = versionLike.match(/(\d+)(?:\.(\d+))?(?:\.(\d+))?/);
  if (!match) return null;

  const major = Number(match[1]);
  const minor = match[2] === undefined ? 0 : Number(match[2]);
  const patch = match[3] === undefined ? 0 : Number(match[3]);

  // Weighted so the three tiers never overlap: any 1.x outranks every 0.y,
  // and any 0.y outranks every 0.0.z.
  if (major > 0) return major * 1e12;
  if (minor > 0) return minor * 1e6;
  return patch;
}

function validateExceptions() {
  for (const [registry, exceptions] of Object.entries(inlineExceptions)) {
    if (!exceptions || typeof exceptions !== "object" || Array.isArray(exceptions)) {
      throw new Error(`Inline ${registry} exceptions must be an object.`);
    }

    for (const [name, exception] of Object.entries(exceptions)) {
      if (
        !exception ||
        typeof exception.reason !== "string" ||
        exception.reason.trim() === "" ||
        typeof exception.exitCondition !== "string" ||
        exception.exitCondition.trim() === ""
      ) {
        throw new Error(
          `Inline ${registry} exception for ${name} must include a non-empty reason and exitCondition.`,
        );
      }
    }
  }
}

async function fetchJson(url, description) {
  let lastError;

  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt += 1) {
    try {
      const response = await fetch(url, {
        headers: {
          accept: "application/json",
          "user-agent": "devboule-v2-direct-dependency-check",
        },
        signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
      });

      if (!response.ok) {
        throw new Error(`HTTP ${response.status} ${response.statusText}`);
      }

      return JSON.parse(await response.text());
    } catch (error) {
      lastError = error;
      if (attempt < MAX_ATTEMPTS) {
        console.warn(
          `RETRY ${description}: attempt ${attempt}/${MAX_ATTEMPTS} failed (${errorMessage(error)}).`,
        );
        await sleep(RETRY_DELAY_MS * 2 ** (attempt - 1));
      }
    }
  }

  throw new Error(
    `${description} failed after ${MAX_ATTEMPTS} attempts: ${errorMessage(lastError)}`,
  );
}

async function runWithConcurrency(items, worker, concurrency = 8) {
  const results = new Array(items.length);
  let nextIndex = 0;

  async function runWorker() {
    while (nextIndex < items.length) {
      const index = nextIndex;
      nextIndex += 1;
      results[index] = await worker(items[index]);
    }
  }

  const workerCount = Math.min(concurrency, items.length);
  await Promise.all(Array.from({ length: workerCount }, () => runWorker()));
  return results;
}

function readNpmDependencies() {
  const packageJson = JSON.parse(readFileSync(resolve(repoRoot, "package.json"), "utf8"));
  const dependencies = [];

  for (const section of ["dependencies", "devDependencies"]) {
    for (const [name, declaration] of Object.entries(packageJson[section] ?? {})) {
      const declaredMajor = parseMajor(declaration);
      if (declaredMajor === null) {
        throw new Error(
          `Cannot determine the declared major for npm ${name} from ${JSON.stringify(declaration)}.`,
        );
      }

      dependencies.push({
        registry: "npm",
        name,
        section,
        declaration,
        declaredMajor,
        exception: inlineExceptions.npm[name],
      });
    }
  }

  return dependencies.sort((left, right) => left.name.localeCompare(right.name));
}

async function readCargoDependencies() {
  let metadata;

  try {
    const result = await execFileAsync(
      "cargo",
      ["metadata", "--no-deps", "--format-version", "1", "--locked"],
      {
        cwd: repoRoot,
        maxBuffer: 16 * 1024 * 1024,
        windowsHide: true,
      },
    );
    metadata = JSON.parse(result.stdout);
  } catch (error) {
    throw new Error(`cargo metadata failed: ${errorMessage(error)}`);
  }

  const expectedManifests = new Map(
    [
      ["src-tauri/Cargo.toml", "src-tauri"],
      ["crates/devboule-protocol/Cargo.toml", "crates/devboule-protocol"],
      ["crates/devboule-daemon/Cargo.toml", "crates/devboule-daemon"],
    ].map(([manifest, label]) => [resolve(repoRoot, manifest).toLowerCase(), label]),
  );
  const dependencies = [];
  const foundManifests = new Set();

  for (const packageInfo of metadata.packages ?? []) {
    const manifestLabel = expectedManifests.get(resolve(packageInfo.manifest_path).toLowerCase());
    if (!manifestLabel) continue;
    foundManifests.add(manifestLabel);

    for (const dependency of packageInfo.dependencies ?? []) {
      const crateName = dependency.rename ?? dependency.name;
      const isCratesIoDependency = dependency.source === CRATES_IO_SOURCE;

      dependencies.push({
        registry: "cargo",
        name: crateName,
        declaredAs: dependency.name,
        manifestLabel,
        declaration: dependency.req,
        declaredMajor: parseMajor(dependency.req),
        isCratesIoDependency,
        exception: inlineExceptions.cargo[crateName],
      });
    }
  }

  const missingManifests = [...expectedManifests.values()].filter(
    (manifestLabel) => !foundManifests.has(manifestLabel),
  );
  if (missingManifests.length > 0) {
    throw new Error(
      `cargo metadata did not report the required manifest(s): ${missingManifests.join(", ")}.`,
    );
  }

  for (const dependency of dependencies) {
    if (dependency.isCratesIoDependency && dependency.declaredMajor === null) {
      throw new Error(
        `Cannot determine the declared major for Cargo ${dependency.name} from ${JSON.stringify(dependency.declaration)}.`,
      );
    }
  }

  return dependencies.sort((left, right) =>
    `${left.name}:${left.manifestLabel}`.localeCompare(`${right.name}:${right.manifestLabel}`),
  );
}

async function checkNpmDependency(dependency) {
  const metadata = await fetchJson(npmRegistryUrl(dependency.name), `npm ${dependency.name}`);
  const latestVersion = metadata.version;
  const latestMajor = parseMajor(latestVersion);
  if (latestMajor === null) {
    throw new Error(`npm latest response has no usable version: ${JSON.stringify(latestVersion)}`);
  }

  return {
    ...dependency,
    latestVersion,
    latestMajor,
    behind: dependency.declaredMajor < latestMajor,
  };
}

async function checkCargoDependency(dependency) {
  if (!dependency.isCratesIoDependency) {
    return { ...dependency, status: "non-registry" };
  }

  const metadata = await fetchJson(
    cargoRegistryUrl(dependency.name),
    `crates.io ${dependency.name}`,
  );
  const maxStableVersion = metadata.crate?.max_stable_version;

  if (maxStableVersion === null || maxStableVersion === undefined) {
    return { ...dependency, status: "no-stable-release" };
  }

  const latestMajor = parseMajor(maxStableVersion);
  if (latestMajor === null) {
    throw new Error(
      `crates.io response for ${dependency.name} has no usable max_stable_version: ${JSON.stringify(maxStableVersion)}`,
    );
  }

  return {
    ...dependency,
    latestVersion: maxStableVersion,
    latestMajor,
    behind: dependency.declaredMajor < latestMajor,
    status: "checked",
  };
}

function printNpmResult(result) {
  if (result.error) {
    console.error(`ERROR npm ${result.name}: ${result.error}`);
    return;
  }

  const prefix = result.behind && result.exception ? "EXCEPTION" : result.behind ? "FAIL" : "OK";
  const suffix = result.exception
    ? `; reason: ${result.exception.reason}; exit condition: ${result.exception.exitCondition}`
    : "";
  console.log(
    `${prefix} npm ${result.name}: ${result.declaration} -> latest ${result.latestVersion}${suffix}`,
  );
}

function printCargoResult(result) {
  if (result.error) {
    console.error(`ERROR Cargo ${result.name} (${result.manifestLabel}): ${result.error}`);
    return;
  }

  if (result.status === "non-registry") {
    console.log(
      `SKIP Cargo ${result.name} (${result.manifestLabel}): ${result.declaration} is a local or non-crates.io dependency; no crates.io comparison applies.`,
    );
    return;
  }

  if (result.status === "no-stable-release") {
    console.log(
      `NO STABLE Cargo ${result.name} (${result.manifestLabel}): crates.io max_stable_version is null/absent; no stable major comparison applies.`,
    );
    return;
  }

  const prefix = result.behind && result.exception ? "EXCEPTION" : result.behind ? "FAIL" : "OK";
  const suffix = result.exception
    ? `; reason: ${result.exception.reason}; exit condition: ${result.exception.exitCondition}`
    : "";
  console.log(
    `${prefix} Cargo ${result.name} (${result.manifestLabel}): ${result.declaration} -> max stable ${result.latestVersion}${suffix}`,
  );
}

async function main() {
  validateExceptions();

  const npmDependencies = readNpmDependencies();
  const cargoDependencies = await readCargoDependencies();
  const registryDependencies = [
    ...npmDependencies.map((dependency) => ({ ...dependency, check: checkNpmDependency })),
    ...cargoDependencies
      .filter((dependency) => dependency.isCratesIoDependency)
      .map((dependency) => ({ ...dependency, check: checkCargoDependency })),
  ];

  console.log(
    `Checking ${npmDependencies.length} npm and ${cargoDependencies.length} Cargo direct dependencies against current registry metadata...`,
  );

  const results = await runWithConcurrency(registryDependencies, async (dependency) => {
    try {
      return await dependency.check(dependency);
    } catch (error) {
      return { ...dependency, error: errorMessage(error) };
    }
  });
  const resultsByKey = new Map(
    results.map((result) => [
      `${result.registry}:${result.name}:${result.manifestLabel ?? result.section}`,
      result,
    ]),
  );

  for (const dependency of npmDependencies) {
    printNpmResult(
      resultsByKey.get(`npm:${dependency.name}:${dependency.section}`) ?? {
        ...dependency,
        error: "No registry result was produced.",
      },
    );
  }

  for (const dependency of cargoDependencies) {
    printCargoResult(
      resultsByKey.get(`cargo:${dependency.name}:${dependency.manifestLabel}`) ?? {
        ...dependency,
        status: dependency.isCratesIoDependency ? undefined : "non-registry",
        error: dependency.isCratesIoDependency ? "No registry result was produced." : undefined,
      },
    );
  }

  const failures = results.filter((result) => result.error);
  const behind = results.filter((result) => result.behind && !result.exception);
  const cargoRegistryCount = cargoDependencies.filter(
    (dependency) => dependency.isCratesIoDependency,
  ).length;
  const cargoNonRegistryCount = cargoDependencies.length - cargoRegistryCount;
  if (failures.length > 0 || behind.length > 0) {
    if (failures.length > 0) {
      console.error(
        `Dependency major-version check failed: ${failures.length} registry request(s) could not be checked.`,
      );
    }
    if (behind.length > 0) {
      console.error(
        `Dependency major-version check failed: ${behind.length} direct dependenc${behind.length === 1 ? "y is" : "ies are"} behind by an incompatible version.`,
      );
    }
    process.exitCode = 1;
    return;
  }

  console.log(
    `Dependency major-version check passed: ${npmDependencies.length} npm and ${cargoRegistryCount} Cargo registry dependencies checked; ${cargoNonRegistryCount} Cargo local/non-registry dependencies reported separately.`,
  );
}

try {
  await main();
} catch (error) {
  console.error(`Dependency major-version check could not run: ${errorMessage(error)}`);
  process.exitCode = 1;
}
