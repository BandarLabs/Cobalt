#!/usr/bin/env python3
"""Render Settings review fixtures without changing the workspace lockfile.

The normal app tests use only the public SDK. Pixel capture additionally needs
the renderer and font crates, so compile the same source in a temporary package
whose dependencies and lockfile cannot change the Store release baseline.
"""

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
    app = root / 'examples/settings'
    workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]
    if not os.environ.get("COBALT_REVIEW_OUT"):
        raise SystemExit("Set COBALT_REVIEW_OUT to an output directory")
    environment = os.environ.copy()
    environment["COBALT_REVIEW_OUT"] = str(
        Path(environment["COBALT_REVIEW_OUT"]).resolve()
    )
    environment.setdefault("CARGO_TARGET_DIR", str(root / "target"))
    with tempfile.TemporaryDirectory(prefix="cobalt-settings-review-") as temporary:
        package = Path(temporary)
        source = package / "src"
        source.mkdir()
        for name in ("main.rs", "list_tests.rs"):
            shutil.copyfile(app / "src" / name, source / name)
        shutil.copyfile(review.parent.parent / "src/review_capture.rs", source / "review_capture.rs")
        with (source / "main.rs").open("a") as main_source:
            main_source.write("\n#[cfg(test)]\nmod review_capture;\n")
        render = package / "screenshots" / "ui-review"
        render.mkdir(parents=True)
        shutil.copyfile(review / "render.rs", render / "render.rs")
        lines = [
            "[workspace]",
            "[package]",
            'name = "kobo-settings-ui-review"',
            f'version = {json.dumps(workspace["package"]["version"])}',
            f'edition = {json.dumps(workspace["package"]["edition"])}',
            "publish = false",
            "[dependencies]",
        ]
        for name in (
            "kobo-app-store", "kobo-json", "kobo-sdk", "kobo-ui", "kobo-text"
        ):
            path = json.dumps(str(root / "crates" / name))
            features = (
                ', features = ["runtime-settings"]' if name == "kobo-sdk" else ""
            )
            lines.append(f"{name} = {{ path = {path}{features} }}")
        (package / "Cargo.toml").write_text("\n".join(lines) + "\n")
        # Preserve the workspace's resolved third-party versions. Cargo prunes
        # and updates only this temporary copy for the capture package.
        shutil.copyfile(root / "Cargo.lock", package / "Cargo.lock")
        subprocess.run(
            [
                "cargo", "test", "--offline", "--manifest-path",
                str(package / "Cargo.toml"),
                "capture_review_screens", "--", "--ignored",
            ],
            cwd=root,
            env=environment,
            check=True,
        )


if __name__ == "__main__":
    main()
