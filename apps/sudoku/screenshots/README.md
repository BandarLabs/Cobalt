# Sudoku screenshots

Actual SDK app in the Clara BW simulator at 1072 × 1448 with extra-large interface text. These are ideal grayscale simulator frames, not physical-reader captures. All puzzles are original and bundled with the app.

| Image | Source and provenance |
| --- | --- |
| game.png | [Pencil notes](../../../docs/quality/evidence/sudoku/02-pencil-notes.json) |
| completed.png | [Completed puzzle](../../../docs/quality/evidence/sudoku/09-completed-puzzle.json) |
| recovery.png | [Failed-save recovery](../../../docs/quality/evidence/sudoku/05-save-recovery.json) |
| landscape.png | [Lower landscape rows](../../../docs/quality/evidence/sudoku/15-landscape-lower.json) |

The public app page uses `game.png` at `docs/media/site/apps/sudoku.png`. Each source capture records binary and source digests, fonts, profile, text scale, orientation and fixture identity. Landscape captures retain the physical panel orientation recorded by the CLI.

Build `kobo-cli` from this checkout, then run `scripts/quality/check-sudoku-sim.py --output /tmp/sudoku-check --scale extra-large` using the same cargo target directory. Capture metadata records dirty source accurately when a documentation update follows validation.
