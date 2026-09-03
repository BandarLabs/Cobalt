import test from "node:test";
import assert from "node:assert/strict";
import {
  affectsDevicePackage,
  devicePackageChanges,
  nextPatchVersion,
  unpublishedChangeReport
} from "./device-package-changes.mjs";

test("packaged source, manifests and toolchain pins reach readers only in a release", () => {
  for (const path of [
    "crates/kobo-protocol/src/lib.rs",
    "apps/backgammon/src/main.rs",
    "apps/backgammon/Cargo.toml",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    ".cargo/config.toml"
  ]) {
    assert.equal(affectsDevicePackage(path), true, path);
  }
});

test("documentation, marketing assets and site content reach readers without one", () => {
  for (const path of [
    "README.md",
    "docs/index.html",
    "docs/apps/backgammon/index.html",
    "docs/assets/kobo-shot.png",
    "docs/sitemap.xml",
    "kobo-shot.png",
    "CONTRIBUTING.md",
    ".github/workflows/ci.yml",
    "tools/check-app-versions.mjs"
  ]) {
    assert.equal(affectsDevicePackage(path), false, path);
  }
});

test("directory names are matched whole", () => {
  assert.equal(affectsDevicePackage("appstore/notes.md"), false);
  assert.equal(affectsDevicePackage("crates-notes.md"), false);
  assert.equal(affectsDevicePackage("docs/Cargo.toml"), false);
  assert.equal(affectsDevicePackage("docs/rust-toolchain.toml"), false);
});

test("a mixed push reports only the paths that need a release", () => {
  assert.deepEqual(
    devicePackageChanges([
      "docs/index.html",
      "crates/kobo-ui/src/list.rs",
      "README.md",
      "Cargo.lock",
      "crates/kobo-ui/src/list.rs"
    ]),
    ["Cargo.lock", "crates/kobo-ui/src/list.rs"]
  );
  assert.deepEqual(devicePackageChanges(["docs/index.html", "README.md"]), []);
});

test("the report names the exact version bump that publishes the change", () => {
  const report = unpublishedChangeReport("beta-v0.3.5", "0.3.5", ["Cargo.lock"]);
  assert.match(report, /beta-v0\.3\.5 is already published/);
  assert.match(report, /^ {2}Cargo\.lock$/m);
  assert.match(report, /from 0\.3\.5 to 0\.3\.6/);
  assert.match(report, /publish beta-v0\.3\.6/);
});

test("version arithmetic refuses anything that is not a release number", () => {
  assert.equal(nextPatchVersion("0.3.9"), "0.3.10");
  assert.equal(nextPatchVersion("1.0.0"), "1.0.1");
  assert.throws(() => nextPatchVersion("0.3"), /invalid workspace version 0\.3/);
  assert.throws(() => nextPatchVersion("0.3.5-beta"), /invalid workspace version/);
});
