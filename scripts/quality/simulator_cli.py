"""Build and identify the checkout's CLI before collecting simulator evidence."""

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


def build_cli(root, target):
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
    dirty = bool(subprocess.check_output(
        ["git", "status", "--porcelain", "--untracked-files=no"], cwd=root, text=True))
    return cli, dict(source_revision=revision, tracked_changes=dirty,
                     cli_sha256=fingerprint(cli), toolchain="1.85.1")


def verify_cli(cli, provenance):
    if fingerprint(cli) != provenance["cli_sha256"]:
        raise RuntimeError("CLI changed during capture; rerun with a dedicated CARGO_TARGET_DIR")
