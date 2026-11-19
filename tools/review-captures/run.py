#!/usr/bin/env python3
"""Run a relocated review_capture.rs fixture in an isolated Cargo package."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib


def value(item):
    if isinstance(item, dict):
        return '{ ' + ', '.join(f'{key} = {value(val)}' for key, val in item.items()) + ' }'
    return json.dumps(item)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', help='Repository-relative app, e.g. apps/parser')
    parser.add_argument('--out', required=True, type=Path)
    parser.add_argument('--filter', default='capture_')
    args = parser.parse_args()
    review = Path(__file__).resolve().parent
    root = review.parent.parent
    app = (root / args.app).resolve()
    app.relative_to(root)
    if not (review / args.app / 'src/review_capture.rs').is_file():
        parser.error('No relocated review_capture fixture for this app')
    manifest = tomllib.loads((app / 'Cargo.toml').read_text())
    workspace = tomllib.loads((root / 'Cargo.toml').read_text())['workspace']
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    environment = dict(os.environ)
    for name in ['COBALT_REVIEW_OUT', 'COBALT_REVIEW_CAPTURE_DIR', 'COBALT_UI_CAPTURE_DIR']:
        environment[name] = str(out)
    environment.setdefault('CARGO_TARGET_DIR', str(root / 'target'))
    with tempfile.TemporaryDirectory(prefix='cobalt-review-capture-') as temporary:
        mirror = Path(temporary)
        package = mirror / args.app
        shutil.copytree(app, package)
        shutil.copytree(review / args.app, package, dirs_exist_ok=True)
        (mirror / 'scripts').symlink_to(root / 'scripts', target_is_directory=True)
        main = package / 'src/main.rs'
        main.write_text(main.read_text() + '\n#[cfg(test)]\nmod review_capture;\n')
        lines = ['[workspace]', '[package]']
        for key in ['name', 'version', 'edition', 'rust-version']:
            item = manifest['package'].get(key)
            if item is None:
                continue
            if isinstance(item, dict) and item.get('workspace'):
                item = workspace['package'][key]
            lines.append(f'{key} = {value(item)}')
        lines.extend(['publish = false', '[dependencies]'])
        dependencies = dict(manifest.get('dependencies', {}))
        dependencies.update(manifest.get('dev-dependencies', {}))
        for name in ['kobo-ui', 'kobo-text', 'kobo-image']:
            dependencies.setdefault(name, {'path': str(root / 'crates' / name)})
        for name, dependency in dependencies.items():
            if isinstance(dependency, dict) and 'path' in dependency:
                dependency = dict(dependency)
                dependency['path'] = str((app / dependency['path']).resolve())
            lines.append(f'{name} = {value(dependency)}')
        (package / 'Cargo.toml').write_text('\n'.join(lines) + '\n')
        shutil.copyfile(root / 'Cargo.lock', package / 'Cargo.lock')
        subprocess.run(['cargo', 'test', '--offline', '--manifest-path', str(package / 'Cargo.toml'), args.filter, '--', '--include-ignored', '--nocapture'], cwd=root, env=environment, check=True)


if __name__ == '__main__':
    main()
