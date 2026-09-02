import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import {
  changedLockPackageIdentities,
  changedRegistryPackages,
  checkEntries,
  checkProtocolMinimums,
  compatibleChangePaths,
  lockfileOnlyAddsPackages,
  manifestOnlyChangesPathDependencyVersions,
  manifestOnlyChangesWorkspaceMembershipOrVersion,
  packagesToBuild,
  registeredConsumers,
  releaseDiffArguments,
  releaseNeeded,
  releaseDependencyIds
} from "./check-app-versions.mjs";

function fixture({ currentVersion = "1.0.0", summary = "Summary" } = {}) {
  const app = {
    package: "kobo-notes",
    id: "notes",
    display_name: "Notes",
    short_label: "Notes",
    summary,
    version: currentVersion,
    minimum_cobalt_version: "0.3.0",
    glyph: "note",
    capabilities: ["network"]
  };
  const previous = {
    format_version: 1,
    id: "notes",
    display_name: "Notes",
    short_label: "Notes",
    summary: "Summary",
    version: "1.0.0",
    minimum_cobalt_version: "0.3.0",
    glyph: "note",
    capabilities: ["network"],
    binary_sha256: "0".repeat(64),
    binary_bytes: 3
  };
  return {
    registry: { format_version: 1, apps: [app] },
    published: { format_version: 1, entries: [{ manifest: previous }] }
  };
}

test("accepts an unchanged app at the published version", () => {
  const values = fixture();
  assert.doesNotThrow(() => checkEntries(values.registry, values.published, new Set()));
});

test("requires a version bump when code or a dependency changes", () => {
  const values = fixture();
  assert.throws(
    () => checkEntries(values.registry, values.published, new Set(["kobo-notes"])),
    /package inputs changed \(release inputs\).*version 1\.0\.0 is not newer than 1\.0\.0/s
  );
});

test("requires a version bump when public metadata changes", () => {
  const values = fixture({ summary: "New summary" });
  assert.throws(
    () => checkEntries(values.registry, values.published, new Set()),
    /package inputs changed \(summary\).*version 1\.0\.0 is not newer than 1\.0\.0/s
  );
});

test("accepts changed content with a new version", () => {
  const values = fixture({ currentVersion: "1.0.1", summary: "New summary" });
  assert.doesNotThrow(() =>
    checkEntries(values.registry, values.published, new Set(["kobo-notes"]))
  );
});

test("rejects downgraded and nonnumeric release versions", () => {
  const downgrade = fixture({ currentVersion: "0.9.0", summary: "New summary" });
  assert.throws(
    () => checkEntries(downgrade.registry, downgrade.published, new Set()),
    /version 0\.9\.0 is not newer than 1\.0\.0/
  );

  const token = fixture({ currentVersion: "next", summary: "New summary" });
  assert.throws(
    () => checkEntries(token.registry, token.published, new Set()),
    /version next is not newer than 1\.0\.0/
  );
});

test("matches runtime numeric version ordering", () => {
  const values = fixture({ currentVersion: "1.0.0.1", summary: "New summary" });
  assert.doesNotThrow(() => checkEntries(values.registry, values.published, new Set()));

  values.registry.apps[0].version = "1.0.0.0";
  assert.throws(
    () => checkEntries(values.registry, values.published, new Set()),
    /version 1\.0\.0\.0 is not newer than 1\.0\.0/
  );

  values.registry.apps[0].version = "18446744073709551615.0";
  assert.doesNotThrow(() => checkEntries(values.registry, values.published, new Set()));

  values.registry.apps[0].version = "18446744073709551616.0";
  assert.throws(
    () => checkEntries(values.registry, values.published, new Set()),
    /version 18446744073709551616\.0 is not newer than 1\.0\.0/
  );
});

test("selective builds include changed binaries manifests and new apps only", () => {
  const values = fixture();
  assert.deepEqual(packagesToBuild(values.registry, values.published, new Set()), []);
  assert.deepEqual(
    packagesToBuild(values.registry, values.published, new Set(["kobo-notes"])),
    ["kobo-notes"]
  );

  values.registry.apps[0].version = "1.0.1";
  assert.deepEqual(packagesToBuild(values.registry, values.published, new Set()), ["kobo-notes"]);

  values.registry.apps.push({
    ...values.registry.apps[0],
    id: "reader",
    package: "kobo-reader"
  });
  assert.deepEqual(packagesToBuild(values.registry, values.published, new Set()), [
    "kobo-notes",
    "kobo-reader"
  ]);
});

test("catalog removal still requires publication without a build", () => {
  const values = fixture();
  values.registry.apps = [];
  assert.equal(releaseNeeded(values.registry, values.published, new Set()), true);
});

test("changing an app ID to a different Cargo package is a release input", () => {
  const previous = {
    format_version: 1,
    apps: [{ id: "notes", package: "kobo-notes" }]
  };
  const current = {
    format_version: 1,
    apps: [{ id: "notes", package: "kobo-reader" }]
  };
  assert.deepEqual(changedRegistryPackages(previous, current), new Set(["kobo-reader"]));

  const values = fixture();
  values.registry.apps[0].package = "kobo-reader";
  assert.throws(
    () => checkEntries(values.registry, values.published, new Set(["kobo-reader"])),
    /package inputs changed \(release inputs\).*version 1\.0\.0 is not newer than 1\.0\.0/s
  );
  assert.equal(
    releaseNeeded(values.registry, values.published, new Set(["kobo-reader"])),
    true
  );
});

test("rejects a minimum Cobalt release older than the package protocol", () => {
  const values = fixture();
  values.registry.apps[0].minimum_cobalt_version = "0.2.3";
  assert.throws(
    () => checkProtocolMinimums(values.registry, 10, new Map([[10, "0.2.4"]])),
    /minimum Cobalt 0\.2\.3 is older than protocol 10, first supported by 0\.2\.4/
  );
});

test("accepts the first Cobalt release supporting the package protocol", () => {
  const values = fixture();
  values.registry.apps[0].minimum_cobalt_version = "0.2.4";
  assert.doesNotThrow(() =>
    checkProtocolMinimums(values.registry, 10, new Map([[10, "0.2.4"]]))
  );
});

test("manifest-only rebuilds must meet the current protocol minimum", () => {
  const values = fixture({ currentVersion: "1.0.1", summary: "New summary" });
  const built = new Set(packagesToBuild(values.registry, values.published, new Set()));
  assert.deepEqual([...built], ["kobo-notes"]);
  assert.throws(
    () => checkProtocolMinimums(values.registry, 12, new Map([[12, "0.3.5"]]), built),
    /minimum Cobalt 0\.3\.0 is older than protocol 12/
  );
});

test("new packages must meet the current protocol minimum", () => {
  const values = fixture();
  values.registry.apps.push({
    ...values.registry.apps[0],
    package: "kobo-reader",
    id: "reader",
    display_name: "Reader",
    short_label: "Reader"
  });
  const built = new Set(packagesToBuild(values.registry, values.published, new Set()));
  assert.deepEqual([...built], ["kobo-reader"]);
  assert.throws(
    () => checkProtocolMinimums(values.registry, 12, new Map([[12, "0.3.5"]]), built),
    /reader: minimum Cobalt 0\.3\.0 is older than protocol 12/
  );
});

test("release inputs ignore exclusively dev-only dependency edges", () => {
  const dependencies = releaseDependencyIds({
    deps: [
      { pkg: "normal", dep_kinds: [{ kind: null, target: null }] },
      { pkg: "build", dep_kinds: [{ kind: "build", target: null }] },
      { pkg: "dev-only", dep_kinds: [{ kind: "dev", target: null }] },
      {
        pkg: "normal-and-dev",
        dep_kinds: [
          { kind: "dev", target: null },
          { kind: null, target: "cfg(unix)" }
        ]
      },
      // Fail conservatively if older Cargo metadata omits dependency kinds.
      { pkg: "unspecified", dep_kinds: [] }
    ]
  });

  assert.deepEqual(dependencies, ["normal", "build", "normal-and-dev", "unspecified"]);
});

test("release input discovery includes deleted paths", () => {
  assert.deepEqual(releaseDiffArguments("published"), [
    "diff",
    "--name-only",
    "--diff-filter=ACDMRT",
    "published...HEAD"
  ]);
});

test("workspace version and member additions do not change existing app release inputs", () => {
  const previous = `[workspace]\nmembers = [\n    "apps/notes",\n]\nresolver = "2"\n\n[workspace.package]\nversion = "0.3.1"\nedition = "2021"\n`;
  const current = `[workspace]\nmembers = [\n    "apps/notes",\n    "apps/reader",\n]\nresolver = "2"\n\n[workspace.package]\nversion = "0.3.2"\nedition = "2021"\n`;

  assert.equal(manifestOnlyChangesWorkspaceMembershipOrVersion(previous, current), true);
});

test("workspace configuration changes still affect every app", () => {
  const previous = `[workspace]\nmembers = [\n    "apps/notes",\n]\nresolver = "2"\n\n[workspace.package]\nversion = "0.3.1"\n`;
  const current = `[workspace]\nmembers = [\n    "apps/notes",\n    "apps/reader",\n]\nresolver = "3"\n\n[workspace.package]\nversion = "0.3.2"\n`;

  assert.equal(manifestOnlyChangesWorkspaceMembershipOrVersion(previous, current), false);
});

test("path dependency version-only manifest edits are not app release inputs", () => {
  const previous = `[dependencies]\nkobo-sdk = { version = "0.3.1", path = "../kobo-sdk", features = ["text"] }\n`;
  const current = previous.replace('version = "0.3.1"', 'version = "0.3.2"');
  assert.equal(manifestOnlyChangesPathDependencyVersions(previous, current), true);
  assert.equal(
    manifestOnlyChangesPathDependencyVersions(
      previous,
      current.replace('features = ["text"]', 'features = ["runtime-settings"]')
    ),
    false
  );
});

test("only exact reviewed compatible blobs are excluded from app release inputs", () => {
  const manifest = {
    format_version: 1,
    changes: [
      {
        protocol_version: 11,
        reason: "additive runtime setting",
        files: [
          {
            path: "crates/kobo-protocol/src/lib.rs",
            base_blob: "a".repeat(40),
            compatible_blob: "b".repeat(40)
          }
        ]
      }
    ]
  };
  const changed = ["crates/kobo-protocol/src/lib.rs"];
  assert.deepEqual(
    compatibleChangePaths(
      manifest,
      11,
      changed,
      () => "a".repeat(40),
      () => "b".repeat(40)
    ),
    new Set(changed)
  );
  assert.deepEqual(
    compatibleChangePaths(
      manifest,
      11,
      changed,
      () => "a".repeat(40),
      () => "c".repeat(40)
    ),
    new Set()
  );
  assert.deepEqual(
    compatibleChangePaths(
      manifest,
      12,
      changed,
      () => "a".repeat(40),
      () => "b".repeat(40)
    ),
    new Set()
  );
  assert.throws(
    () =>
      compatibleChangePaths(
        {
          ...manifest,
          changes: [
            {
              ...manifest.changes[0],
              files: [
                {
                  ...manifest.changes[0].files[0],
                  path: "apps/arxiv/src/main.rs"
                }
              ]
            }
          ]
        },
        11,
        ["apps/arxiv/src/main.rs"],
        () => "a".repeat(40),
        () => "b".repeat(40)
      ),
    /invalid app release compatible-change file/
  );
});

test("reviewed compatible-change entries name the exact current files", () => {
  const manifest = JSON.parse(
    readFileSync("tools/app-release-compatible-changes.json", "utf8")
  );
  for (const change of manifest.changes) {
    for (const file of change.files) {
      assert.equal(
        execFileSync("git", ["hash-object", file.path], { encoding: "utf8" }).trim(),
        file.compatible_blob,
        `${file.path} changed without reviewing its Store release impact`
      );
    }
  }
});

test("new lockfile package blocks do not change existing app release inputs", () => {
  const previous = `version = 4\n\n[[package]]\nname = "notes"\nversion = "1.0.0"\n`;
  const current = `${previous}\n[[package]]\nname = "reader"\nversion = "1.0.0"\n`;

  assert.equal(lockfileOnlyAddsPackages(previous, current), true);
});

function metadata() {
  const workspacePackage = name => ({
    id: `${name} 1.0.0`,
    name,
    version: "0.3.2",
    source: null
  });
  const registryPackage = name => ({
    id: `${name} 1.0.0`,
    name,
    version: "1.0.0",
    source: "registry+https://example.test/index"
  });
  const dependency = pkg => ({ pkg, dep_kinds: [{ kind: null, target: null }] });
  return {
    packages: [
      workspacePackage("notes"),
      workspacePackage("reader"),
      workspacePackage("weather"),
      registryPackage("notes-dep"),
      registryPackage("shared"),
      registryPackage("unrelated")
    ],
    resolve: {
      nodes: [
        { id: "notes 1.0.0", deps: [dependency("notes-dep 1.0.0"), dependency("shared 1.0.0")] },
        { id: "reader 1.0.0", deps: [dependency("notes 1.0.0")] },
        { id: "weather 1.0.0", deps: [dependency("shared 1.0.0")] },
        { id: "notes-dep 1.0.0", deps: [] },
        { id: "shared 1.0.0", deps: [] },
        { id: "unrelated 1.0.0", deps: [] }
      ]
    }
  };
}

function registryIdentity(name, version = "1.0.0") {
  return JSON.stringify([name, version, "registry+https://example.test/index"]);
}

test("an app-local lock change affects that app and its true dependents only", () => {
  const previous = `version = 4\n\n[[package]]\nname = "notes-dep"\nversion = "1.0.0"\nsource = "registry+https://example.test/index"\nchecksum = "old"\n`;
  const current = previous.replace('checksum = "old"', 'checksum = "new"');
  const changed = changedLockPackageIdentities(previous, current);
  assert.deepEqual(changed, new Set([registryIdentity("notes-dep")]));
  assert.deepEqual(
    registeredConsumers(metadata(), ["notes", "reader", "weather"], changed),
    new Set(["notes", "reader"])
  );
});

test("adding a package outside every Store app closure affects no app", () => {
  const previous = `version = 4\n\n[[package]]\nname = "notes"\nversion = "0.3.1"\n`;
  const current = `${previous}\n[[package]]\nname = "unrelated"\nversion = "1.0.0"\nsource = "registry+https://example.test/index"\nchecksum = "new"\n`;
  const changed = changedLockPackageIdentities(previous, current);
  assert.deepEqual(changed, new Set([registryIdentity("unrelated")]));
  assert.deepEqual(
    registeredConsumers(metadata(), ["notes", "reader", "weather"], changed),
    new Set()
  );
});

test("a shared dependency lock change affects every consuming Store app", () => {
  assert.deepEqual(
    registeredConsumers(
      metadata(),
      ["notes", "reader", "weather"],
      new Set([registryIdentity("shared")])
    ),
    new Set(["notes", "reader", "weather"])
  );
});

test("a changed dependency version does not affect consumers of another version", () => {
  const values = metadata();
  values.packages.push(
    {
      id: "png 0.17.16",
      name: "png",
      version: "0.17.16",
      source: "registry+https://example.test/index"
    },
    {
      id: "png 0.18.1",
      name: "png",
      version: "0.18.1",
      source: "registry+https://example.test/index"
    }
  );
  values.resolve.nodes.find(node => node.id === "notes 1.0.0").deps.push({
    pkg: "png 0.17.16",
    dep_kinds: [{ kind: null, target: null }]
  });
  values.resolve.nodes.find(node => node.id === "weather 1.0.0").deps.push({
    pkg: "png 0.18.1",
    dep_kinds: [{ kind: null, target: null }]
  });
  values.resolve.nodes.push(
    { id: "png 0.17.16", deps: [] },
    { id: "png 0.18.1", deps: [] }
  );

  assert.deepEqual(
    registeredConsumers(
      values,
      ["notes", "reader", "weather"],
      new Set([registryIdentity("png", "0.18.1")])
    ),
    new Set(["weather"])
  );
});

test("a lock change with no identifiable current package fails closed", () => {
  assert.throws(
    () =>
      registeredConsumers(
        metadata(),
        ["notes", "reader", "weather"],
        new Set([registryIdentity("removed-dependency")])
      ),
    /cannot identify its consumers/
  );
});

test("workspace package version-only lock changes affect no Store app", () => {
  const previous = `version = 4\n\n[[package]]\nname = "kobo-sdk"\nversion = "0.3.1"\ndependencies = [\n "shared",\n]\n`;
  const current = previous.replace('version = "0.3.1"', 'version = "0.3.2"');
  assert.deepEqual(changedLockPackageIdentities(previous, current), new Set());
});
