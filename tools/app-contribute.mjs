#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { collectRegistry, currentProtocolVersion, normalizeContribution } from "./app-registry.mjs";

const scriptPath = fileURLToPath(import.meta.url);
const root = resolve(dirname(scriptPath), "..");

function run(name, args, errorHint, environment = process.env) {
  try {
    execFileSync(name, args, { cwd: root, stdio: "inherit", env: environment });
  } catch {
    throw new Error(`${name} ${args.join(" ")} failed. ${errorHint}`);
  }
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

export function contributionPlan(manifestPath) {
  const absolute = resolve(root, manifestPath);
  const appDirectory = dirname(absolute);
  const directoryName = basename(appDirectory);
  const raw = JSON.parse(readFileSync(absolute, "utf8"));
  const app = normalizeContribution(raw, directoryName);
  const cargoPath = join(appDirectory, "Cargo.toml");
  const cargo = readFileSync(cargoPath, "utf8");
  const packageMatch = /^\s*name\s*=\s*"([^"]+)"\s*$/m.exec(cargo);
  if (!packageMatch) {
    throw new Error(`${relative(root, cargoPath)} has no [package] name`);
  }
  if (packageMatch[1] !== app.package) {
    throw new Error(
      `${relative(root, cargoPath)} package '${packageMatch[1]}' must be '${app.package}'`
    );
  }
  const registry = collectRegistry();
  const registered = registry.apps.find(candidate => candidate.id === app.id);
  if (!registered || JSON.stringify(registered) !== JSON.stringify(app)) {
    throw new Error(
      `${relative(root, absolute)} is not the effective registry entry; run the collector and fix duplicate or stale metadata`
    );
  }
  return {
    app,
    manifestPath: relative(root, absolute),
    cargoPath: relative(root, cargoPath),
    protocolVersion: currentProtocolVersion(),
    registry
  };
}

export function manifestForBinary(app, binary) {
  return {
    format_version: 1,
    id: app.id,
    display_name: app.display_name,
    short_label: app.short_label,
    summary: app.summary,
    version: app.version,
    minimum_cobalt_version: app.minimum_cobalt_version,
    glyph: app.glyph,
    capabilities: app.capabilities,
    binary_sha256: sha256(binary),
    binary_bytes: binary.length
  };
}

function main(args) {
  const manifestIndex = args.indexOf("--manifest");
  const dryRun = args.includes("--dry-run");
  const printPlan = args.includes("--print-plan");
  const allowed = new Set(["--manifest", "--dry-run", "--print-plan"]);
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (!allowed.has(argument) && args[index - 1] !== "--manifest") {
      throw new Error(
        "usage: node tools/app-contribute.mjs --manifest apps/ID/cobalt-app.json --dry-run"
      );
    }
  }
  if (manifestIndex < 0 || !args[manifestIndex + 1]) {
    throw new Error(
      "usage: node tools/app-contribute.mjs --manifest apps/ID/cobalt-app.json --dry-run"
    );
  }
  const plan = contributionPlan(args[manifestIndex + 1]);
  if (printPlan) {
    console.log(JSON.stringify(plan));
    return;
  }
  if (!dryRun) {
    throw new Error(
      "app contributions have no local publish mode; pass --dry-run and let protected Beta automation sign and publish"
    );
  }

  const output = join(root, "target", "app-contribute", plan.app.id);
  const binaryDirectory = join(output, "binary");
  const previewDirectory = join(output, "preview");
  mkdirSync(binaryDirectory, { recursive: true });
  mkdirSync(previewDirectory, { recursive: true });
  const registryPath = join(output, "registry.json");
  writeFileSync(registryPath, `${JSON.stringify(plan.registry, null, 2)}\n`);

  run(
    "node",
    ["tools/generate-app-pages.mjs"],
    "Fix the manifest or generated-page error, then retry; do not hand-edit generated pages."
  );
  run("cargo", ["fmt", "--all", "--", "--check"], "Run cargo fmt --all, then retry.");
  run(
    "cargo",
    ["test", "--locked", "-p", plan.app.package],
    `Fix ${plan.app.package} tests, then retry.`
  );
  run(
    "cargo",
    ["clippy", "--locked", "-p", plan.app.package, "--all-targets", "--all-features", "--", "-D", "warnings"],
    `Fix ${plan.app.package} clippy findings, then retry.`
  );
  run(
    "cargo",
    [
      "run",
      "--locked",
      "-p",
      "kobo-cli",
      "--",
      "app-check",
      "--registry",
      registryPath,
      "--package",
      plan.app.package,
      "--out",
      binaryDirectory
    ],
    "Install an ARMv7 hard-float cross compiler (gcc-arm-linux-gnueabihf on Debian), then retry.",
    { ...process.env, CARGO_TARGET_DIR: join(root, "target") }
  );

  const binaryPath = join(binaryDirectory, plan.app.package);
  const binary = readFileSync(binaryPath);
  const releaseManifest = manifestForBinary(plan.app, binary);
  const releaseManifestPath = join(previewDirectory, "manifest.json");
  writeFileSync(releaseManifestPath, JSON.stringify(releaseManifest));
  const seed = join(root, "tools", "fixtures", "beta-store-smoke", "fixture-seed.hex");
  const packagePath = join(previewDirectory, `${plan.app.id}-${plan.app.version}.cobalt-app`);
  run(
    "cargo",
    [
      "run", "--locked", "-q", "-p", "kobo-cli", "--", "app-bundle",
      "--manifest", releaseManifestPath,
      "--binary", binaryPath,
      "--seed", seed,
      "--out", packagePath
    ],
    "The local pathless package could not be built."
  );
  run(
    "cargo",
    [
      "run", "--locked", "-q", "-p", "kobo-cli", "--", "app-catalog",
      "--seed", seed,
      "--out", join(previewDirectory, "cobalt-app-catalog.json"),
      "--signature", join(previewDirectory, "cobalt-app-catalog.json.sig"),
      "--entry", packagePath,
      `https://example.invalid/releases/download/app-catalog-beta/${basename(packagePath)}`
    ],
    "The local signed catalog preview could not be built."
  );
  writeFileSync(
    join(previewDirectory, "LOCAL-ONLY.txt"),
    "Signed with Cobalt's public deterministic fixture key. CI rebuilds and signs protected Beta artifacts.\n"
  );
  writeFileSync(
    join(output, "summary.json"),
    `${JSON.stringify({
      format_version: 1,
      app_id: plan.app.id,
      package: plan.app.package,
      protocol_version: plan.protocolVersion,
      derived_minimum_cobalt_version: plan.app.minimum_cobalt_version,
      binary_sha256: sha256(binary),
      preview_directory: relative(root, previewDirectory)
    }, null, 2)}\n`
  );
  console.log(
    `contribution check passed for ${plan.app.id}; local Beta-parity preview: ${relative(root, previewDirectory)}`
  );
}

if (process.argv[1] && resolve(process.argv[1]) === resolve(scriptPath)) {
  main(process.argv.slice(2));
}
