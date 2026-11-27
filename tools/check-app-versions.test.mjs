import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  COMMAND_MAX_BUFFER,
  changedRegistryPackages,
  checkBuildPackages,
  checkEntries,
  checkProtocolMinimums,
  isContributionManifest,
  isDocumentation,
  isFilmingScript,
  manifestOnlyChangesPathDependencyVersions,
  manifestOnlyChangesVersions,
  appsWithOwnChanges,
  meaningfulReleaseNotes,
  packagesToBuild,
  releaseDiffArguments,
  releaseNeeded,
} from "./check-app-versions.mjs";
import { collectRegistry, currentProtocolVersion, deriveMinimumCobalt } from "./app-registry.mjs";
import {
  registeredStorePackages,
  storeCatalogChanges,
  storeWatchDirectories,
  workspacePackageDirectories
} from "./store-catalog-changes.mjs";

function fixture({
  currentVersion = "1.0.0",
  summary = "Summary",
  releaseNotes
} = {}) {
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
  if (releaseNotes !== undefined) app.release_notes = releaseNotes;
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

test("does not release unchanged apps for a policy-derived minimum bump", () => {
  const values = fixture();
  // Asked of the policy rather than written down. A literal here is a copy of
  // the answer `checkEntries` computes, and the two drift apart the moment the
  // protocol table moves: writing "0.3.5" here made this test fail when
  // protocols 12 and 13 were corrected to 0.3.7, reporting a broken exemption
  // when the exemption was working and only the fixture was stale.
  values.registry.apps[0].minimum_cobalt_version = deriveMinimumCobalt(
    values.registry.apps[0].capabilities
  );
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

test("accepts changed content with a new version and meaningful notes", () => {
  const values = fixture({
    currentVersion: "1.0.1",
    summary: "New summary",
    releaseNotes: "Explain the new summary and improved reader workflow."
  });
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

test("requires release notes only for a new or changed release", () => {
  const unchanged = fixture();
  assert.doesNotThrow(() => checkEntries(unchanged.registry, unchanged.published, new Set()));

  const changed = fixture({ currentVersion: "1.0.1", summary: "New summary" });
  assert.throws(
    () => checkEntries(changed.registry, changed.published, new Set()),
    /needs meaningful release_notes/
  );
  changed.registry.apps[0].release_notes = "Bug fixes";
  assert.throws(
    () => checkEntries(changed.registry, changed.published, new Set()),
    /needs meaningful release_notes/
  );
  changed.registry.apps[0].release_notes =
    "Improves the summary and makes the main task easier to understand.";
  assert.doesNotThrow(() => checkEntries(changed.registry, changed.published, new Set()));

  const added = fixture();
  added.published.entries = [];
  assert.throws(() => checkEntries(added.registry, added.published, new Set()), /new Store app/);
});

test("release note quality rejects placeholders", () => {
  assert.equal(meaningfulReleaseNotes("Bug fixes"), false);
  assert.equal(
    meaningfulReleaseNotes("Fixes duplicate rows when refreshing a long feed."),
    true
  );
});

test("matches runtime numeric version ordering", () => {
  const values = fixture({
    currentVersion: "1.0.0.1",
    summary: "New summary",
    releaseNotes: "Improves the visible summary for readers."
  });
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

// A new app reaches the stable catalog one promotion after it reaches beta.
// While the two disagree, checking a beta-bound change against stable waves the
// app through, and the very next publication to beta rejects it.
test("a new app is version-checked only against a catalog that already lists it", () => {
  const values = fixture();
  const app = {
    package: "kobo-backgammon",
    id: "backgammon",
    display_name: "Backgammon",
    short_label: "Backgammon",
    summary: "Play backgammon",
    version: "0.1.0",
    minimum_cobalt_version: "0.3.1",
    glyph: "dice",
    capabilities: [],
    release_notes: "Play complete solo or pass-and-play backgammon on one Kobo."
  };
  values.registry.apps.push(app);
  const stable = values.published;
  const beta = {
    format_version: 1,
    entries: [
      ...stable.entries,
      {
        manifest: {
          format_version: 1,
          id: app.id,
          display_name: app.display_name,
          short_label: app.short_label,
          summary: app.summary,
          version: app.version,
          minimum_cobalt_version: app.minimum_cobalt_version,
          glyph: app.glyph,
          capabilities: app.capabilities,
          binary_sha256: "1".repeat(64),
          binary_bytes: 5
        }
      }
    ]
  };
  const affected = new Set([app.package]);

  assert.doesNotThrow(() => checkEntries(values.registry, stable, affected));
  assert.throws(
    () => checkEntries(values.registry, beta, affected),
    /backgammon: package inputs changed \(release inputs\).*version 0\.1\.0 is not newer than 0\.1\.0/s
  );
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

test("bounds command output above Cargo's default metadata limit", () => {
  assert.ok(COMMAND_MAX_BUFFER > 1024 * 1024);
  assert.ok(COMMAND_MAX_BUFFER <= 16 * 1024 * 1024);
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

test("the intended Gallery and Zotero build selection passes final validation", () => {
  const values = fixture({
    currentVersion: "1.0.1",
    summary: "Updated Gallery",
    releaseNotes: "Explain the updated Gallery reader workflow."
  });
  values.registry.apps[0].package = "kobo-gallery";
  values.registry.apps[0].id = "gallery";
  values.registry.apps[0].minimum_cobalt_version = "0.3.5";
  values.published.entries[0].manifest.id = "gallery";
  values.registry.apps.push({
    ...values.registry.apps[0],
    package: "kobo-zotero-reader",
    id: "zotero-reader",
    display_name: "Zotero Reader",
    short_label: "Zotero"
  });
  values.published.entries.push({
    manifest: {
      ...values.published.entries[0].manifest,
      id: "zotero-reader",
      display_name: "Zotero Reader",
      short_label: "Zotero"
    }
  });

  assert.doesNotThrow(() =>
    checkBuildPackages(
      values.registry,
      values.published,
      ["kobo-gallery", "kobo-zotero-reader"],
      12,
      new Map([[12, "0.3.5"]])
    )
  );
});

test("fallback expansion rejects a legacy app with an obsolete platform minimum", () => {
  const values = fixture({ currentVersion: "1.0.1", summary: "Updated Gallery" });
  values.registry.apps[0].minimum_cobalt_version = "0.3.5";
  values.registry.apps.push({
    ...values.registry.apps[0],
    package: "kobo-legacy",
    id: "legacy",
    version: "1.0.0",
    minimum_cobalt_version: "0.3.1"
  });
  values.published.entries.push({
    manifest: {
      ...values.published.entries[0].manifest,
      id: "legacy",
      minimum_cobalt_version: "0.3.1"
    }
  });

  assert.throws(
    () =>
      checkBuildPackages(
        values.registry,
        values.published,
        ["kobo-notes", "kobo-legacy"],
        12,
        new Map([[12, "0.3.5"]])
      ),
    /legacy: minimum Cobalt 0\.3\.1 is older than protocol 12/
  );
});

test("fallback expansion rejects rebuilding an unchanged legacy app version", () => {
  const values = fixture({ currentVersion: "1.0.1", summary: "Updated Gallery" });
  values.registry.apps[0].minimum_cobalt_version = "0.3.5";
  values.registry.apps.push({
    ...values.registry.apps[0],
    package: "kobo-legacy",
    id: "legacy",
    version: "1.0.0",
    summary: "Summary"
  });
  values.published.entries.push({
    manifest: {
      ...values.published.entries[0].manifest,
      id: "legacy",
      minimum_cobalt_version: "0.3.5"
    }
  });

  assert.throws(
    () =>
      checkBuildPackages(
        values.registry,
        values.published,
        ["kobo-notes", "kobo-legacy"],
        12,
        new Map([[12, "0.3.5"]])
      ),
    /legacy: package inputs changed \(release inputs\).*version 1\.0\.0 is not newer than 1\.0\.0/s
  );
});

test("apps workflow validates the final package matrix after fallback expansion", () => {
  const workflow = readFileSync(".github/workflows/apps.yml", "utf8");
  const expansion = workflow.lastIndexOf('packages="$all_packages"');
  const validation = workflow.indexOf(
    "node tools/check-app-versions.mjs --validate-packages"
  );
  const outputs = workflow.indexOf('echo "packages=$packages"');

  assert.notEqual(expansion, -1);
  assert.ok(validation > expansion);
  assert.ok(outputs > validation);
  assert.match(
    workflow.slice(validation, outputs),
    /--published-catalog published-app-catalog\.json \\\n\s+--packages "\$packages"/
  );
});

// Asserting the workflow's text kept passing while the command it describes
// exited 1 on every publish run, because no test ever ran it. Read the flags
// back out of the workflow and drive the real CLI with them, so a flag the tool
// does not accept fails here rather than silently skipping app publication.
test("the flags the apps workflow sends are ones the tool actually accepts", () => {
  const workflow = readFileSync(".github/workflows/apps.yml", "utf8");
  const validation = workflow.indexOf(
    "node tools/check-app-versions.mjs --validate-packages"
  );
  const outputs = workflow.indexOf('echo "packages=$packages"');
  const flags = [...workflow.slice(validation, outputs).matchAll(/--[a-z-]+/g)].map(
    match => match[0]
  );
  assert.deepEqual(flags, [
    "--validate-packages",
    "--registry",
    "--published-catalog",
    "--packages"
  ]);

  const registry = collectRegistry();
  const directory = mkdtempSync(join(tmpdir(), "check-app-versions-"));
  const registryPath = join(directory, "generated-app-registry.json");
  const publishedPath = join(directory, "published-app-catalog.json");
  writeFileSync(registryPath, JSON.stringify(registry));
  writeFileSync(publishedPath, '{"format_version":1,"entries":[]}\n');

  const run = packages =>
    execFileSync(
      process.execPath,
      [
        "tools/check-app-versions.mjs",
        "--validate-packages",
        "--registry",
        registryPath,
        "--published-catalog",
        publishedPath,
        "--packages",
        JSON.stringify(packages)
      ],
      { encoding: "utf8" }
    );

  assert.match(run(registry.apps.map(app => app.package)), /Every selected app package/);
  assert.throws(() => run(["kobo-not-registered"]), /unknown build package/);
});

test("release input discovery includes deleted paths", () => {
  assert.deepEqual(releaseDiffArguments("published"), [
    "diff",
    "--name-only",
    "--diff-filter=ACDMRT",
    "published...HEAD"
  ]);
});

test("standalone app metadata is compared as a manifest, not a binary input", () => {
  assert.equal(isContributionManifest("apps/notes/cobalt-app.json", "apps/notes"), true);
  assert.equal(isContributionManifest("apps/notes/src/main.rs", "apps/notes"), false);
});

test("a drive script next to an application is not a release input", () => {
  assert.equal(isFilmingScript("examples/todo/drive.txt", "examples/todo"), true);
  assert.equal(isFilmingScript("examples/todo/drive.kobo", "examples/todo"), true);
  assert.equal(isFilmingScript("examples/gallery/drive.txt", "examples/gallery"), true);
  assert.equal(isFilmingScript("examples/todo/src/main.rs", "examples/todo"), false);
  assert.equal(isFilmingScript("examples/todo/src/drive.txt", "examples/todo"), false);
  assert.equal(isFilmingScript("examples/todo-extra/drive.txt", "examples/todo"), false);
});

// Correcting a stale sentence in apps/syncthing/README.md was refused as an
// unreleased change to the Syncthing application, offering to republish a
// binary to every reader who has it in order to fix a paragraph none of them
// download. Nothing published is built from prose.
test("prose beside an application is not a release input", () => {
  assert.equal(isDocumentation("apps/syncthing/README.md", "apps/syncthing"), true);
  assert.equal(isDocumentation("apps/notes/NOTES.md", "apps/notes"), true);
  // Source is source, whatever it is named.
  assert.equal(isDocumentation("apps/notes/src/main.rs", "apps/notes"), false);
  assert.equal(isDocumentation("apps/notes/build-armv7.sh", "apps/notes"), false);
  // Only prose directly beside the package is excused.
  assert.equal(isDocumentation("apps/notes/docs/guide.md", "apps/notes"), false);
  assert.equal(isDocumentation("apps/notes-extra/README.md", "apps/notes"), false);
});

// The README's own pictures are no more downloaded than its prose. The app
// page publishes from docs/media/site, so a refreshed still beside the package
// must not owe every reader a new version of the app.
test("the screenshots a README shows are not a release input", () => {
  assert.equal(isDocumentation("apps/notes/screenshots/home.png", "apps/notes"), true);
  assert.equal(isDocumentation("examples/todo/screenshots/list.png", "examples/todo"), true);
  // Only that folder, only beside the package.
  assert.equal(isDocumentation("apps/notes/src/screenshots/home.png", "apps/notes"), false);
  assert.equal(isDocumentation("apps/notes/assets/home.png", "apps/notes"), false);
  assert.equal(isDocumentation("apps/notes-extra/screenshots/home.png", "apps/notes"), false);
  assert.equal(isDocumentation("apps/notes/screenshots", "apps/notes"), false);
});

// One route per application was the assumption, and the shelf broke it: an
// application that needs a second scene should not owe the Store a version.
test("every drive script an application grows is still not a release input", () => {
  assert.equal(isFilmingScript("apps/vault/drive-states.kobo", "apps/vault"), true);
  assert.equal(isFilmingScript("apps/frame/drive-empty.kobo", "apps/frame"), true);
  assert.equal(isFilmingScript("apps/inkling/drive.sh", "apps/inkling"), true);
  assert.equal(isFilmingScript("apps/lichess/drive/game.kobo", "apps/lichess"), true);
  assert.equal(isFilmingScript("apps/lichess/drive", "apps/lichess"), true);

  // Still only beside the package, and still not its source.
  assert.equal(isFilmingScript("apps/vault/src/drive-states.kobo", "apps/vault"), false);
  assert.equal(isFilmingScript("apps/vault/driver.rs", "apps/vault"), false);
  assert.equal(isFilmingScript("apps/vault-extra/drive.sh", "apps/vault"), false);
});

// The tree is the thing that regressed, so check it rather than a fixture.
test("no drive script in this tree counts as a release input", () => {
  const packages = [
    ...readdirSync("apps", { withFileTypes: true }).map(e => ["apps", e]),
    ...readdirSync("examples", { withFileTypes: true }).map(e => ["examples", e])
  ].filter(([, entry]) => entry.isDirectory());

  let seen = 0;
  for (const [root, entry] of packages) {
    const directory = `${root}/${entry.name}`;
    for (const file of readdirSync(directory)) {
      if (!file.startsWith("drive")) continue;
      seen += 1;
      assert.equal(
        isFilmingScript(`${directory}/${file}`, directory),
        true,
        `${directory}/${file} would force a version bump on ${entry.name}`
      );
    }
  }
  assert.ok(seen > 30, `expected the shelf's drive scripts, found ${seen}`);
});

// A node the runtime only learned to draw in a later release is a floor on
// which readers may install the app, and the protocol map cannot express it:
// protocol 14 dates from 0.3.12, while the numbered board and the pencil board
// arrived with 0.3.14. Crossword and Logic Pack were installable on 0.3.12 and
// failed when their first board screen was encoded, which is a blank refusal
// on a reader rather than a message anybody can act on. The strokes through
// a checked, wrong letter or digit travel at protocol 18 too, but are drawn
// from 0.3.44, and a runtime before that refuses them.
test("an app drawing a board the runtime learned late says which release it needs", () => {
  const LATE_NODES = [
    ["crossword_board", "0.3.14"],
    ["pencil_board", "0.3.14"],
    ["PencilMarkKind::Candidates", "0.3.18"],
    ["crossword_board_with_struck", "0.3.44"],
    ["PencilMarkKind::WrongDigit", "0.3.44"]
  ];
  const packages = [
    ...readdirSync("apps", { withFileTypes: true }).map(e => ["apps", e]),
    ...readdirSync("examples", { withFileTypes: true }).map(e => ["examples", e])
  ].filter(([, entry]) => entry.isDirectory());

  let checked = 0;
  for (const [root, entry] of packages) {
    const directory = `${root}/${entry.name}`;
    let manifest;
    try {
      manifest = JSON.parse(readFileSync(`${directory}/cobalt-app.json`, "utf8"));
    } catch {
      continue;
    }
    let sources = "";
    const walk = source => {
      for (const file of readdirSync(source, { withFileTypes: true })) {
        const path = `${source}/${file.name}`;
        if (file.isDirectory()) {
          walk(path);
        } else if (file.name.endsWith(".rs")) {
          sources += readFileSync(path, "utf8");
        }
      }
    };
    try {
      walk(`${directory}/src`);
    } catch {
      continue;
    }
    const floors = LATE_NODES.filter(([node]) => sources.includes(`${node}(`));
    if (floors.length === 0) continue;
    checked += 1;
    // The install floor is the newest node the app draws.
    const floor = floors
      .map(([, version]) => version)
      .sort((a, b) => {
        const pa = a.split(".").map(Number);
        const pb = b.split(".").map(Number);
        return pa[0] - pb[0] || pa[1] - pb[1] || pa[2] - pb[2];
      })
      .at(-1);
    assert.equal(
      manifest.minimum_cobalt_version,
      floor,
      `${entry.name} draws ${floors.map(([node]) => node).join(" and ")} and must declare minimum_cobalt_version ${floor}`
    );
  }
  assert.ok(checked >= 2, `expected the board apps, found ${checked}`);
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

test("the standalone importer is isolated without changing device workspace inputs", () => {
  const metadata = manifestPath => JSON.parse(execFileSync("cargo", [
    "metadata", "--locked", "--no-deps", "--format-version", "1",
    ...(manifestPath ? ["--manifest-path", manifestPath] : [])
  ], { encoding: "utf8", maxBuffer: COMMAND_MAX_BUFFER }));
  const workspace = metadata();
  assert.ok(!workspace.workspace_members.some(member => member.includes("kobo-flashcards-import")));
  const importer = metadata("crates/kobo-flashcards-import/Cargo.toml");
  assert.equal(importer.workspace_members.length, 1);
  assert.ok(importer.workspace_members[0].includes("kobo-flashcards-import"));
});

test("a package manifest that only moves its version lines is not a Store change", () => {
  const previous = [
    "[package]",
    'name = "kobo-sudoku"',
    'version = "1.0.11"',
    "edition.workspace = true",
    "",
    "[dependencies]",
    'kobo-sdk = { path = "../../crates/kobo-sdk", version = "0.3.2" }',
    ""
  ].join("\n");
  const inherited = previous.replace('version = "1.0.11"', "version.workspace = true");
  assert.equal(manifestOnlyChangesVersions(previous, inherited), true);
  assert.equal(manifestOnlyChangesVersions(previous, previous.replace('"1.0.11"', '"1.0.15"')), true);
  assert.equal(
    manifestOnlyChangesVersions(previous, inherited.replace('version = "0.3.2"', 'version = "0.3.3"')),
    true
  );
  assert.equal(
    manifestOnlyChangesVersions(previous, inherited.replace("[dependencies]\n", '[dependencies]\nserde = "1"\n')),
    false
  );
  assert.equal(manifestOnlyChangesVersions(previous, inherited.replace("kobo-sudoku", "kobo-sudoku2")), false);
  const dependencyTable = `${previous}\n[dependencies.serde]\nversion = "1"\n`;
  assert.equal(
    manifestOnlyChangesVersions(dependencyTable, dependencyTable.replace('version = "1"\n', 'version = "2"\n')),
    false
  );
});

test("a platform change asks no app for a new version", () => {
  const directories = new Map([
    ["kobo-todo", "examples/todo"],
    ["kobo-backgammon", "apps/backgammon"],
    ["kobo-ui", "crates/kobo-ui"],
    ["kobo-sdk", "crates/kobo-sdk"]
  ]);
  const registered = ["kobo-todo", "kobo-backgammon"];
  const platform = [
    "crates/kobo-ui/src/lib.rs",
    "crates/kobo-sdk/src/lib.rs",
    "crates/kobo-protocol/src/lib.rs",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".cargo/config.toml",
    "tools/protocol-minimums.json"
  ];
  const quiet = () => false;

  assert.deepEqual(appsWithOwnChanges(platform, directories, registered, quiet).affected, new Set());
  // The next app to change is asked for its own version, and only for that:
  // the platform changes beside it are still nobody's.
  assert.deepEqual(
    appsWithOwnChanges(
      [...platform, "apps/backgammon/src/main.rs"],
      directories,
      registered,
      quiet
    ).affected,
    new Set(["kobo-backgammon"])
  );
});

test("an app's own source asks it for a version; its prose, pictures and filming scripts do not", () => {
  const directories = new Map([["kobo-frame", "apps/frame"]]);
  const registered = ["kobo-frame"];
  const quiet = () => false;
  const hostOnly = [
    "apps/frame/drive.kobo",
    "apps/frame/drive/scenes.txt",
    "apps/frame/README.md",
    "apps/frame/screenshots/home.png"
  ];
  assert.deepEqual(appsWithOwnChanges(hostOnly, directories, registered, quiet).affected, new Set());
  for (const source of [
    "apps/frame/src/main.rs",
    "apps/frame/src/drive.txt",
    "apps/frame/src/notes.md",
    "apps/frame/cobalt-app.json",
    "apps/frame/Cargo.toml"
  ]) {
    assert.deepEqual(
      appsWithOwnChanges([...hostOnly, source], directories, registered, quiet).affected,
      new Set(["kobo-frame"]),
      source
    );
  }
  const directoriesWithSibling = new Map([...directories, ["kobo-drive", "apps/drive"]]);
  assert.deepEqual(
    appsWithOwnChanges(["apps/drive/src/main.rs"], directoriesWithSibling, ["kobo-frame"], quiet)
      .affected,
    new Set()
  );
});

test("a package manifest that only moves version numbers asks for nothing", () => {
  const directories = new Map([["kobo-todo", "examples/todo"]]);
  const asked = [];
  const result = appsWithOwnChanges(
    ["examples/todo/Cargo.toml"],
    directories,
    ["kobo-todo"],
    manifest => {
      asked.push(manifest);
      return true;
    }
  );
  assert.deepEqual(asked, ["examples/todo/Cargo.toml"]);
  assert.deepEqual(result.affected, new Set());
  assert.deepEqual(result.quietManifests, new Set(["examples/todo/Cargo.toml"]));
  // A version line beside a source change is part of that change.
  assert.deepEqual(
    appsWithOwnChanges(
      ["examples/todo/Cargo.toml", "examples/todo/src/main.rs"],
      directories,
      ["kobo-todo"],
      () => true
    ).affected,
    new Set(["kobo-todo"])
  );
});

test("a registered app that names no workspace package fails closed", () => {
  assert.throws(
    () => appsWithOwnChanges([], new Map(), ["kobo-ghost"], () => false),
    /kobo-ghost is in the Store catalog but names no workspace member/
  );
});

test("the compatible-change review is gone with the rule that needed it", () => {
  assert.throws(() => readFileSync("tools/app-release-compatible-changes.json", "utf8"), /ENOENT/);
  assert.doesNotMatch(readFileSync("tools/check-app-versions.mjs", "utf8"), /compatible-change/);
});
