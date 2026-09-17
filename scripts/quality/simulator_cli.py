"""Build and identify the checkout's CLI before collecting simulator evidence."""

# Evidence is harness output: sidecars carry per-run timings, and a live
# feed drifts between runs. Changes confined to the evidence tree therefore
# say nothing about the sources under test, and every harness excludes it
# from the build-time dirt check by default.

import hashlib
import json
import os
import subprocess
from pathlib import Path


def fingerprint(path):
    digest = hashlib.sha256()
    with path.open("rb") as binary:
        for chunk in iter(lambda: binary.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def build_cli(root, target, ignore_prefix="docs/quality/evidence"):
    build = subprocess.run(
        ["cargo", "+1.85.1", "build", "-p", "kobo-cli", "--message-format=json"],
        cwd=root,
        env=dict(os.environ, CARGO_TARGET_DIR=str(target),
                 CARGO_PROFILE_DEV_DEBUG="0", CARGO_INCREMENTAL="0", CARGO_BUILD_JOBS="1"),
        stdout=subprocess.PIPE, text=True, check=True,
    )
    executables = []
    for line in build.stdout.splitlines():
        artifact = json.loads(line)
        if (artifact.get("reason") == "compiler-artifact"
                and artifact.get("target", {}).get("name") == "kobo"
                and artifact.get("executable")):
            executables.append(Path(artifact["executable"]).resolve())
    if len(executables) != 1:
        raise RuntimeError("Cargo did not identify exactly one kobo executable")
    cli = executables[0]
    revision = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    status = subprocess.check_output(
        ["git", "status", "--porcelain", "--untracked-files=no"], cwd=root, text=True)
    if ignore_prefix is not None:
        # A harness writes its evidence into its own output directory while
        # it runs, and a previous run's committed evidence may legitimately
        # differ (a live feed drifts). Changes confined there say nothing
        # about the sources under test, so they do not taint provenance.
        prefix = ignore_prefix.rstrip("/") + "/"
        status = "\n".join(
            line for line in status.splitlines()
            if not line[3:].startswith(prefix))
    dirty = bool(status.strip())
    return cli, dict(source_revision=revision, tracked_changes=dirty,
                     cli_sha256=fingerprint(cli), toolchain="1.85.1")


def verify_cli(cli, provenance):
    if fingerprint(cli) != provenance["cli_sha256"]:
        raise RuntimeError("CLI changed during capture; rerun with a dedicated CARGO_TARGET_DIR")
