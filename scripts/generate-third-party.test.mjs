import { describe, expect, it, vi } from "vitest";

import {
  cargoMetadataFailure,
  collectCargoRecords,
  collectNpmRecords,
  generateFile,
  parsePnpmPackageEntries,
  readCargoMetadata,
  renderDocument,
  replaceInventorySections,
  unifiedDiff,
  writeGeneratedDocument,
} from "./generate-third-party.mjs";

const rustRecords = [
  {
    name: "z crate",
    version: "2.0.0",
    kind: "Rust transitive (lockfile)",
    license: "MIT OR Apache-2.0",
  },
  {
    name: "a crate",
    version: "1.0.0",
    kind: "Rust transitive (lockfile)",
    license: "BSD-3-Clause AND MIT",
  },
];

const npmRecords = [
  { name: "z-package", version: "2.0.0", kind: "npm transitive", license: "MIT" },
  { name: "a-package", version: "1.0.0", kind: "npm transitive", license: "Apache-2.0 OR MIT" },
];

const documentFixture = [
  "# Third-party notices",
  "",
  "Human prose before the generated inventory.",
  "The Rust inventory is the complete set of 2 registry packages.",
  "The JavaScript inventory is the complete set of 2 package records in pnpm-lock.yaml.",
  "The Rust count is 2 rather than the 4 entries in `Cargo.lock` because workspace crates are not third-party.",
  "",
  "## Complete resolved inventory",
  "",
  "### Rust registry packages (Cargo.lock; 1 records)",
  "",
  "| Name | Version | Kind | Licence |",
  "| old | old | old | old |",
  "",
  "### npm packages (pnpm-lock.yaml; 1 records)",
  "",
  "| Name | Version | Kind | Licence |",
  "| old | old | old | old |",
  "",
  "## License text and redistribution",
  "",
  "Human license prose must survive unchanged.",
  "",
].join("\n");

const npmLockFixture = [
  "lockfileVersion: '9.0'",
  "",
  "packages:",
  "",
  "  foo@1.0.0:",
  "    resolution: {integrity: sha512-test}",
  "",
  "snapshots:",
  "",
  "  foo@1.0.0: {}",
  "",
].join("\n");

function cargoPackage(id, name, source = "registry+https://github.com/rust-lang/crates.io-index") {
  return { id, name, version: "1.0.0", source, license: "MIT" };
}

const cargoLabelMetadata = {
  workspace_members: ["path+file:///workspace#root"],
  packages: [
    {
      id: "path+file:///workspace#root",
      name: "workspace",
      version: "0.1.0",
      source: null,
      license: "Apache-2.0",
      dependencies: [
        { name: "portable-pty", optional: true, target: null, kind: null },
        { name: "windows-sys", optional: false, target: "cfg(windows)", kind: null },
        {
          name: "linux-only",
          optional: true,
          target: 'cfg(not(any(target_os = "macos", target_os = "windows")))',
          kind: null,
        },
        {
          name: "multi",
          optional: true,
          target: 'cfg(not(any(target_os = "macos", target_os = "windows")))',
          kind: null,
        },
        { name: "multi", optional: true, target: 'cfg(target_os = "windows")', kind: null },
      ],
    },
    cargoPackage(
      "registry+https://github.com/rust-lang/crates.io-index#portable-pty@1.0.0",
      "portable-pty",
    ),
    cargoPackage(
      "registry+https://github.com/rust-lang/crates.io-index#windows-sys@1.0.0",
      "windows-sys",
    ),
    cargoPackage(
      "registry+https://github.com/rust-lang/crates.io-index#linux-only@1.0.0",
      "linux-only",
    ),
    cargoPackage("registry+https://github.com/rust-lang/crates.io-index#multi@1.0.0", "multi"),
  ],
  resolve: {
    nodes: [
      {
        id: "path+file:///workspace#root",
        deps: [
          {
            name: "portable_pty",
            pkg: "registry+https://github.com/rust-lang/crates.io-index#portable-pty@1.0.0",
            dep_kinds: [{ kind: null, target: null }],
          },
          {
            name: "windows_sys",
            pkg: "registry+https://github.com/rust-lang/crates.io-index#windows-sys@1.0.0",
            dep_kinds: [{ kind: null, target: "cfg(windows)" }],
          },
          {
            name: "linux_only",
            pkg: "registry+https://github.com/rust-lang/crates.io-index#linux-only@1.0.0",
            dep_kinds: [
              { kind: null, target: 'cfg(not(any(target_os = "macos", target_os = "windows")))' },
            ],
          },
          {
            name: "multi",
            pkg: "registry+https://github.com/rust-lang/crates.io-index#multi@1.0.0",
            dep_kinds: [
              { kind: null, target: 'cfg(not(any(target_os = "macos", target_os = "windows")))' },
              { kind: null, target: 'cfg(target_os = "windows")' },
            ],
          },
        ],
      },
    ],
  },
};

describe("generate-third-party", () => {
  it("inserts markers on the first run and preserves prose outside both sections", () => {
    const first = replaceInventorySections(documentFixture, {
      rust: rustRecords,
      npm: npmRecords,
    });

    expect(first.migrationMessages).toHaveLength(2);
    expect(first.markdown).toContain("<!-- BEGIN GENERATED THIRD-PARTY RUST -->");
    expect(first.markdown).toContain("<!-- END GENERATED THIRD-PARTY NPM -->");
    expect(first.markdown).toContain("Human prose before the generated inventory.");
    expect(first.markdown).toContain("Human license prose must survive unchanged.");

    const second = replaceInventorySections(first.markdown, {
      rust: rustRecords,
      npm: npmRecords,
    });
    expect(second.migrationMessages).toHaveLength(0);
    expect(second.markdown).toBe(first.markdown);
  });

  it("refuses first-run migration when it would discard a human note", () => {
    const unsafe = documentFixture.replace(
      "| old | old | old | old |\n\n### npm",
      "| old | old | old | old |\n\nHuman note: this row is patched locally.\n\n### npm",
    );
    expect(() => replaceInventorySections(unsafe, { rust: rustRecords, npm: npmRecords })).toThrow(
      /rust migration would discard human content.*Human note/,
    );
  });

  it("ignores marker examples inside markdown fences", () => {
    const fenced = [
      "# Third-party notices",
      "",
      "```markdown",
      "<!-- BEGIN GENERATED THIRD-PARTY RUST -->",
      "### Rust registry packages (example)",
      "<!-- END GENERATED THIRD-PARTY RUST -->",
      "```",
      "",
      "## Complete resolved inventory",
      "",
      "### Rust registry packages (Cargo.lock; 1 records)",
      "",
      "| Name | Version | Kind | Licence |",
      "| old | old | old | old |",
      "",
      "### npm packages (pnpm-lock.yaml; 1 records)",
      "",
      "| Name | Version | Kind | Licence |",
      "| old | old | old | old |",
      "",
      "## License text and redistribution",
    ].join("\n");
    const result = replaceInventorySections(fenced, { rust: rustRecords, npm: npmRecords });
    expect(result.markdown).toContain("```markdown\n<!-- BEGIN GENERATED THIRD-PARTY RUST -->");
    expect(result.markdown.match(/<!-- BEGIN GENERATED THIRD-PARTY RUST -->/gu)).toHaveLength(2);
    expect(result.markdown).toContain("### Rust registry packages (Cargo.lock; 2 records)");
  });

  it("renders the same bytes regardless of input record order", () => {
    const first = renderDocument(documentFixture, {
      rustRecords,
      npmRecords,
      cargoPackageCount: 4,
    }).markdown;
    const second = renderDocument(documentFixture, {
      rustRecords: [...rustRecords].reverse(),
      npmRecords: [...npmRecords].reverse(),
      cargoPackageCount: 4,
    }).markdown;

    expect(second).toBe(first);
    expect(first.indexOf("| a crate |")).toBeLessThan(first.indexOf("| z crate |"));
    expect(first).toContain("2 registry packages");
    expect(first).toContain("2 package records in pnpm-lock.yaml");
  });

  it("fails instead of rewriting stale human-owned count prose", () => {
    const stale = documentFixture.replace(
      "complete set of 2 registry",
      "complete set of 3 registry",
    );
    expect(() => renderDocument(stale, { rustRecords, npmRecords, cargoPackageCount: 4 })).toThrow(
      /Human-owned count prose is stale/,
    );
  });

  it("fails with the package name when Cargo metadata omits a license", () => {
    expect(() =>
      collectCargoRecords({
        workspace_members: ["workspace#root 0.1.0"],
        packages: [
          {
            id: "registry+https://github.com/rust-lang/crates.io-index#bad 1.0.0",
            name: "bad-crate",
            version: "1.0.0",
            source: "registry+https://github.com/rust-lang/crates.io-index",
            license: null,
          },
        ],
        resolve: { nodes: [] },
      }),
    ).toThrow("bad-crate@1.0.0");
  });

  it("classifies normalized Cargo names, multiple targets, and negative cfg correctly", () => {
    const records = collectCargoRecords(cargoLabelMetadata);
    const kinds = new Map(records.map((record) => [record.name, record.kind]));
    expect(kinds.get("portable-pty")).toBe("Rust direct runtime optional");
    expect(kinds.get("windows-sys")).toBe("Rust direct Windows");
    expect(kinds.get("linux-only")).toBe("Rust direct runtime optional");
    expect(kinds.get("multi")).toBe("Rust direct optional Windows");
  });

  it("fails rather than silently excluding git-sourced Cargo packages", () => {
    expect(() =>
      collectCargoRecords({
        workspace_members: [],
        packages: [
          cargoPackage(
            "git+https://example.test/repo#git-crate@1.0.0",
            "git-crate",
            "git+https://example.test/repo",
          ),
        ],
        resolve: { nodes: [] },
      }),
    ).toThrow(/git-crate@1.0.0.*git\+https:\/\/example\.test\/repo/);
  });

  it("fails with the complete key on nested peer suffixes", () => {
    const nestedKey =
      "vitest@5.0.0(@types/node@26.4.0)(happy-dom@20.13.2)(vite@8.2.2(@types/node@26.4.0))";
    const lock = [
      "packages:",
      "",
      `  '${nestedKey}':`,
      "    resolution: {}",
      "",
      "snapshots:",
      "",
    ].join("\n");
    expect(() => parsePnpmPackageEntries(lock)).toThrow(nestedKey);
  });

  it("treats libc-only package gates as platform packages", () => {
    const lock = [
      "packages:",
      "",
      "  musl-only@1.0.0:",
      "    libc: [musl]",
      "",
      "snapshots:",
      "",
    ].join("\n");
    expect(parsePnpmPackageEntries(lock)[0].isPlatform).toBe(true);
  });

  it("keeps non-workspace source-null crates and excludes workspace members", () => {
    const records = collectCargoRecords({
      workspace_members: ["workspace#root 0.1.0"],
      packages: [
        {
          id: "workspace#root 0.1.0",
          name: "devboule",
          version: "0.1.0",
          source: null,
          license: "Apache-2.0",
        },
        {
          id: "path+file:///vendor/esaxx-rs#0.1.10",
          name: "esaxx-rs",
          version: "0.1.10",
          source: null,
          license: "Apache-2.0",
        },
      ],
      resolve: { nodes: [] },
    });

    expect(records).toEqual([
      {
        name: "esaxx-rs",
        version: "0.1.10",
        kind: "Rust vendored third-party",
        license: "Apache-2.0",
      },
    ]);
  });

  it("uses the production npm path and exact-version fetch metadata", async () => {
    const fetchImpl = vi.fn(async (url) => ({
      ok: true,
      status: 200,
      statusText: "OK",
      json: async () => ({ name: "foo", version: "1.0.0", license: "MIT" }),
      url,
    }));
    const records = await collectNpmRecords(npmLockFixture, { fetchImpl });
    expect(records).toEqual([
      { name: "foo", version: "1.0.0", kind: "npm transitive", license: "MIT" },
    ]);
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    expect(fetchImpl.mock.calls[0][0]).toContain("foo/1.0.0");
  });

  it("fails the production npm path with the package name on HTTP and license errors", async () => {
    await expect(
      collectNpmRecords(npmLockFixture, {
        fetchImpl: async () => ({ ok: false, status: 503, statusText: "Unavailable" }),
      }),
    ).rejects.toThrow("foo@1.0.0");
    await expect(
      collectNpmRecords(npmLockFixture, {
        fetchImpl: async () => ({
          ok: true,
          status: 200,
          statusText: "OK",
          json: async () => ({ name: "foo", version: "1.0.0" }),
        }),
      }),
    ).rejects.toThrow("foo@1.0.0");
  });

  it("fails clearly when Cargo metadata exceeds the stdout limit", async () => {
    const error = Object.assign(new Error("stdout maxBuffer length exceeded"), {
      code: "ERR_CHILD_PROCESS_STDIO_MAXBUFFER",
    });
    await expect(readCargoMetadata(async () => Promise.reject(error))).rejects.toThrow("128 MiB");
    expect(cargoMetadataFailure(error).message).toContain("128 MiB");
  });

  it("logs migration warnings before writing", () => {
    const events = [];
    const changed = writeGeneratedDocument(
      "fixture.md",
      "old",
      { markdown: "new", migrationMessages: ["MIGRATE: safe"] },
      {
        log: () => events.push("log"),
        write: () => events.push("write"),
      },
    );
    expect(changed).toBe(true);
    expect(events).toEqual(["log", "write"]);
  });

  it("lets the production --check branch distinguish synchronized and stale content", async () => {
    const generated = renderDocument(documentFixture, {
      rustRecords,
      npmRecords,
      cargoPackageCount: 4,
    }).markdown;

    let current = generated;
    const readText = (path) => (String(path).endsWith("pnpm-lock.yaml") ? npmLockFixture : current);
    const options = {
      target: "fixture/THIRD_PARTY.md",
      check: true,
      readText,
      cargoMetadataReader: async () => ({
        workspace_members: ["path+file:///workspace#one", "path+file:///workspace#two"],
        packages: [
          {
            id: "path+file:///workspace#one",
            name: "workspace-one",
            version: "0.1.0",
            source: null,
            license: "Apache-2.0",
          },
          {
            id: "path+file:///workspace#two",
            name: "workspace-two",
            version: "0.1.0",
            source: null,
            license: "Apache-2.0",
          },
          {
            id: "registry+https://github.com/rust-lang/crates.io-index#a%20crate@1.0.0",
            name: "a crate",
            version: "1.0.0",
            source: "registry+https://github.com/rust-lang/crates.io-index",
            license: "BSD-3-Clause AND MIT",
          },
          {
            id: "registry+https://github.com/rust-lang/crates.io-index#z%20crate@2.0.0",
            name: "z crate",
            version: "2.0.0",
            source: "registry+https://github.com/rust-lang/crates.io-index",
            license: "MIT OR Apache-2.0",
          },
        ],
        resolve: { nodes: [] },
      }),
      npmRecordsCollector: async () => npmRecords,
    };

    await expect(generateFile(options)).resolves.toMatchObject({ changed: false });

    current = generated.replace("| a crate |", "| changed |", 1);
    await expect(generateFile(options)).resolves.toMatchObject({ changed: true });
    expect(unifiedDiff(current, generated, "fixture/THIRD_PARTY.md")).toContain("+| a crate |");
  });
});
