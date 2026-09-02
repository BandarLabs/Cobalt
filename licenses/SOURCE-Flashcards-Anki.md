# Corresponding source for the Flashcards host converter

The host-only `flashcards-import` executable links Anki Rust library code from
this exact source revision:

- Repository: <https://github.com/ankitects/anki>
- Commit: `9e32ad8849068510a82273889c21b22e1acf0949`
- Linked packages: `anki` (rslib), `anki_i18n`, `anki_io`, and `anki_proto`
- Licence: AGPL-3.0-or-later, with the upstream notice and complete AGPL text
  in `LICENSE-Anki.txt`

Retrieve the corresponding upstream source without selecting a moving branch:

```sh
git clone --filter=blob:none --no-checkout \
  https://github.com/ankitects/anki.git anki-source
git -C anki-source checkout \
  9e32ad8849068510a82273889c21b22e1acf0949
test "$(git -C anki-source rev-parse HEAD)" = \
  9e32ad8849068510a82273889c21b22e1acf0949
```

The complete Cobalt-side corresponding source is the entire
<https://github.com/BandarLabs/Cobalt> repository at the source commit recorded
beside a distributed artifact, not a subset of directories. That checkout
includes every local workspace crate, build script, embedded notice, font,
licence file, and artifact-generation script used by the helper.

The repository artifact builder embeds that exact 40-character Cobalt commit
in `flashcards-import --licenses` and writes the same value to
`flashcards-import.source-commit.txt`. The artifact audit requires both values
to match the checkout being audited.

`Cargo.lock` records the same immutable Anki Git revision and all resolved Rust
dependencies. The host dependency notice lists their exact package versions
and licences; `cargo vendor --locked vendor` materializes their corresponding
registry/Git source into one local directory when an offline source archive is
required.

To use the documented artifact-audit layout, build the host helper from the
full repository checkout with:

```sh
target_root=/path/to/flashcards-target-root
mkdir -p "$target_root/artifacts"
test -z "$(git status --porcelain --untracked-files=normal)"
source_commit=$(git rev-parse HEAD)
COBALT_SOURCE_COMMIT="$source_commit" \
CARGO_TARGET_DIR="$target_root/host-target" \
  cargo build --locked --release -p kobo-flashcards-import
printf '%s\n' "$source_commit" > \
  "$target_root/artifacts/flashcards-import.source-commit.txt"
```

No Anki source is copied into the Kobo application. The neutral `CBFLASH`
bundle is Cobalt-owned; the host converter is the only artifact that links the
pinned Anki code.
