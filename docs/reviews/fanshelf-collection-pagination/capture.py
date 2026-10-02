#!/usr/bin/env python3
"""Capture unchanged app source in a temporary Cargo package, not the workspace.

Python 3.11+. Renderer-only dependencies and lockfile edits stay temporary.
--baseline uses the preserved before driver and original 97153048 app source.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", action="store_true")
    args = parser.parse_args()
    review = Path(__file__).resolve().parent
    root = review.parents[2]
    app = root / "apps" / "fanshelf"
    workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]
    manifest = tomllib.loads((app / "Cargo.toml").read_text())
    environment = os.environ.copy()
    if not environment.get("COBALT_REVIEW_CAPTURE_DIR"):
        raise SystemExit("Set COBALT_REVIEW_CAPTURE_DIR to an output directory")
    environment["COBALT_REVIEW_CAPTURE_DIR"] = str(
        Path(environment["COBALT_REVIEW_CAPTURE_DIR"]).resolve()
    )
    environment.setdefault("CARGO_TARGET_DIR", str(root / "target"))
    with tempfile.TemporaryDirectory(prefix="cobalt-fanshelf-review-") as temporary:
        package = Path(temporary)
        source = package / "src"
        source.mkdir()
        if args.baseline:
            for name in ("main.rs", "library.rs"):
                original = subprocess.check_output(
                    ["git", "show", f"97153048:apps/fanshelf/src/{name}"], cwd=root
                )
                (source / name).write_bytes(original)
        else:
            for file in (app / "src").glob("*.rs"):
                shutil.copyfile(file, source / file.name)
        driver = "baseline-capture.rs" if args.baseline else "capture.rs"
        shutil.copyfile(review / driver, source / "review_capture.rs")
        with (source / "main.rs").open("a") as main_source:
            main_source.write("\n#[cfg(test)]\nmod review_capture;\n")
        lines = [
            "[workspace]", "[package]", 'name = "kobo-fanshelf"',
            f'version = {json.dumps(manifest["package"]["version"])}',
            f'edition = {json.dumps(workspace["package"]["edition"])}',
            "publish = false", "[dependencies]",
        ]
        for name in (
            "kobo-bookview", "kobo-read", "kobo-sdk", "kobo-xml",
            "kobo-ui", "kobo-text", "kobo-image",
        ):
            path = json.dumps(str(root / "crates" / name))
            lines.append(f"{name} = {{ path = {path} }}")
        (package / "Cargo.toml").write_text("\n".join(lines) + "\n")
        shutil.copyfile(root / "Cargo.lock", package / "Cargo.lock")
        subprocess.run(
            ["cargo", "test", "--offline", "--manifest-path",
             str(package / "Cargo.toml"), "capture_review_collections", "--", "--nocapture"],
            cwd=root, env=environment, check=True,
        )


if __name__ == "__main__":
    main()
