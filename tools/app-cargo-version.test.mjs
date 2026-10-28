import test from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { cargoVersionFaults, packageVersion, storePackages } from "./app-cargo-version.mjs";

const WORKSPACE = `[package]
name = "kobo-example"
version.workspace = true
edition.workspace = true

[dependencies]
kobo-sdk = { path = "../../crates/kobo-sdk", version = "0.3.2" }
`;

test("the package version is read in every spelling and only from [package]", () => {
  assert.deepEqual(packageVersion(WORKSPACE), { workspace: true });
  assert.deepEqual(
    packageVersion(WORKSPACE.replace("version.workspace = true", "version = { workspace = true }")),
    { workspace: true }
  );
  assert.deepEqual(
    packageVersion(WORKSPACE.replace("version.workspace = true", 'version = "1.0.11"')),
    { workspace: false, value: "1.0.11" }
  );
  assert.equal(packageVersion(WORKSPACE.replace("version.workspace = true\n", "")), null);
  assert.equal(packageVersion('[dependencies.serde]\nversion = "1"\n'), null);
});

test("a compliant manifest reads as compliant whatever comments and table forms it uses", () => {
  const inherited = { workspace: true };
  assert.deepEqual(
    packageVersion(WORKSPACE.replace("version.workspace = true", "version.workspace = true # platform version")),
    inherited
  );
  assert.deepEqual(packageVersion(WORKSPACE.replace("[package]", "[package] # the Store app")), inherited);
  assert.deepEqual(packageVersion(WORKSPACE.replace("[package]", "[ package ]")), inherited);
  assert.deepEqual(
    packageVersion(WORKSPACE.replace("version.workspace = true", "version . workspace = true")),
    inherited
  );
  assert.deepEqual(
    packageVersion(
      WORKSPACE.replace("version.workspace = true\n", "") + "\n[package.version] # inherited\nworkspace = true\n"
    ),
    inherited
  );
  assert.deepEqual(
    packageVersion(WORKSPACE.replace("version.workspace = true", 'version = "1.0.11" # stale copy')),
    { workspace: false, value: "1.0.11" }
  );
  assert.deepEqual(
    packageVersion(WORKSPACE.replace("version.workspace = true", "version = '1.0.11'")),
    { workspace: false, value: "1.0.11" }
  );
});

test("commented-out lines and other package tables are not the package version", () => {
  const bare = WORKSPACE.replace("version.workspace = true\n", "");
  assert.equal(packageVersion(bare.replace("[package]\n", "[package]\n# version.workspace = true\n")), null);
  assert.equal(packageVersion(bare.replace("[package]\n", '[package]\ndescription = "# not a comment"\n')), null);
  assert.equal(packageVersion(bare + '\n[package.metadata.store]\nversion = "2.0.0"\n'), null);
  assert.equal(packageVersion(bare + '\n[[package]]\nversion = "2.0.0"\n'), null);
});

test("a package that repeats a version of its own is named with both numbers", () => {
  const packages = [
    { directory: "apps/right", manifestVersion: "2.0.0", cargo: WORKSPACE },
    {
      directory: "apps/drifted",
      manifestVersion: "1.0.15",
      cargo: WORKSPACE.replace("version.workspace = true", 'version = "1.0.11"')
    },
    {
      directory: "apps/bare",
      manifestVersion: "0.1.0",
      cargo: WORKSPACE.replace("version.workspace = true\n", "")
    }
  ];
  const faults = cargoVersionFaults(packages);
  assert.equal(faults.length, 2);
  assert.match(faults[0], /apps\/drifted\/Cargo\.toml says version = "1\.0\.11" while cobalt-app\.json says 1\.0\.15/);
  assert.match(faults[1], /apps\/bare\/Cargo\.toml has no \[package\] version/);
});

test("packages are found beside a manifest under apps/ and examples/, and nowhere else", () => {
  const root = mkdtempSync(join(tmpdir(), "cobalt-cargo-version-"));
  for (const [directory, manifest] of [
    ["apps/one", true],
    ["apps/plain", false],
    ["examples/two", true],
    ["crates/three", true]
  ]) {
    mkdirSync(join(root, directory), { recursive: true });
    writeFileSync(join(root, directory, "Cargo.toml"), WORKSPACE);
    if (manifest) {
      writeFileSync(join(root, directory, "cobalt-app.json"), JSON.stringify({ version: "1.2.3" }));
    }
  }
  const found = storePackages(root);
  assert.deepEqual(
    found.map(entry => [entry.directory, entry.manifestVersion]),
    [
      ["apps/one", "1.2.3"],
      ["examples/two", "1.2.3"]
    ]
  );
  assert.deepEqual(cargoVersionFaults(found), []);
});

test("every Store package in this tree inherits the workspace version", () => {
  assert.deepEqual(cargoVersionFaults(storePackages()), []);
});
