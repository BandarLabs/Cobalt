#!/usr/bin/env python3
"""Render review fixtures without changing the app's release dependencies."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib


def main():
    review = Path(__file__).resolve().parent
    root = next(parent for parent in review.parents if (parent / "crates/kobo-sdk").is_dir())
    app = root / 'apps/syncthing'
    manifest = tomllib.loads((app / "Cargo.toml").read_text())
    workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]
    if not os.environ.get("COBALT_REVIEW_OUT"):
        raise SystemExit("Set COBALT_REVIEW_OUT to an output directory")
    environment = os.environ.copy()
    environment["COBALT_REVIEW_OUT"] = str(
        Path(environment["COBALT_REVIEW_OUT"]).resolve()
    )
    environment.setdefault("CARGO_TARGET_DIR", str(root / "target"))
    with tempfile.TemporaryDirectory(prefix=f"cobalt-{app.name}-review-") as temporary:
        package = Path(temporary)
        shutil.copytree(app / "src", package / "src")
        if (app / "fixtures").exists():
            shutil.copytree(app / "fixtures", package / "fixtures")
        # Keep the capture's fixture helpers in the same private module as the
        # normal regression tests, without adding capture dependencies there.
        with (package / "src" / "ui_review_tests.rs").open("a") as tests:
            tests.write("\n" + (review / "capture.rs").read_text())
        lines = [
            "[workspace]",
            "[package]",
            f'name = "kobo-{app.name}-ui-review"',
            f'version = {json.dumps(manifest["package"]["version"])}',
            f'edition = {json.dumps(workspace["package"]["edition"])}',
            "publish = false",
            "[dependencies]",
        ]
        dependencies = {
            **manifest.get("dependencies", {}),
            **manifest.get("dev-dependencies", {}),
            "kobo-image": {"path": "../../crates/kobo-image"},
        }
        for name, dependency in dependencies.items():
            path = json.dumps(str((app / dependency["path"]).resolve()))
            options = [f"path = {path}"]
            if "features" in dependency:
                options.append(f'features = {json.dumps(dependency["features"])}')
            if "default-features" in dependency:
                options.append(
                    f'default-features = {json.dumps(dependency["default-features"])}'
                )
            lines.append(f"{name} = {{ {', '.join(options)} }}")
        (package / "Cargo.toml").write_text("\n".join(lines) + "\n")
        # Cargo updates only this temporary copy, preserving resolved versions
        # without adding the PNG encoder to the app's reviewed package record.
        shutil.copyfile(root / "Cargo.lock", package / "Cargo.lock")
        subprocess.run(
            [
                "cargo", "test", "--offline", "--manifest-path",
                str(package / "Cargo.toml"), "capture_review_pages_when_requested",
            ],
            cwd=root,
            env=environment,
            check=True,
        )


if __name__ == "__main__":
    main()
