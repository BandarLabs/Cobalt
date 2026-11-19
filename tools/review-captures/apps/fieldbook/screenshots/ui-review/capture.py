#!/usr/bin/env python3
"""Render real app fixtures in an isolated package, leaving release dependencies intact."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib


def value(item):
    if isinstance(item, dict):
        return "{ " + ", ".join(f"{key} = {value(val)}" for key, val in item.items()) + " }"
    return json.dumps(item)


def main():
    review = Path(__file__).resolve().parent
    root = next(parent for parent in review.parents if (parent / "crates/kobo-sdk").is_dir())
    app = root / 'apps/fieldbook'
    output = os.environ.get("COBALT_REVIEW_CAPTURE_DIR")
    if not output:
        raise SystemExit("Set COBALT_REVIEW_CAPTURE_DIR to an output directory")
    manifest = tomllib.loads((app / "Cargo.toml").read_text())
    workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]
    environment = os.environ.copy()
    environment["COBALT_REVIEW_CAPTURE_DIR"] = str(Path(output).resolve())
    environment.setdefault("CARGO_TARGET_DIR", str(root / "target"))
    environment["CARGO_INCREMENTAL"] = "0"
    with tempfile.TemporaryDirectory(prefix=f"cobalt-{app.name}-capture-") as temporary:
        mirror = Path(temporary)
        package = mirror / app.relative_to(root)
        shutil.copytree(app, package)
        # Capture-only sources live outside the production app tree.
        shutil.copytree(review.parent.parent, package, dirs_exist_ok=True)
        # Some ordinary regression tests include committed shared fixtures.
        (mirror / "scripts").symlink_to(root / "scripts", target_is_directory=True)
        with (package / "src/main.rs").open("a") as main_source:
            main_source.write("\n#[cfg(test)]\nmod review_capture;\n")
        lines = ["[workspace]", "[package]"]
        for key in ("name", "version", "edition", "rust-version"):
            item = manifest["package"][key]
            if isinstance(item, dict) and item.get("workspace"):
                item = workspace["package"][key]
            lines.append(f"{key} = {value(item)}")
        lines.extend(["publish = false", "[dependencies]"])
        dependencies = dict(manifest.get("dependencies", {}))
        dependencies.update(manifest.get("dev-dependencies", {}))
        for name in ("kobo-ui", "kobo-text", "kobo-image"):
            dependencies.setdefault(name, {"path": str(root / "crates" / name)})
        for name, dependency in dependencies.items():
            if isinstance(dependency, dict) and "path" in dependency:
                dependency = dict(dependency)
                dependency["path"] = str((app / dependency["path"]).resolve())
            lines.append(f"{name} = {value(dependency)}")
        (package / "Cargo.toml").write_text("\n".join(lines) + "\n")
        # Only this temporary lock is pruned; the reviewed root lock is untouched.
        shutil.copyfile(root / "Cargo.lock", package / "Cargo.lock")
        subprocess.run(["cargo", "test", "--offline", "--manifest-path", str(package / "Cargo.toml"), "capture_", "--", "--nocapture"], cwd=root, env=environment, check=True)


if __name__ == "__main__":
    main()
