# Nonograms screenshots

Actual SDK output in the Clara BW simulator. These are ideal grayscale frames, not hardware captures.

| Image | Evidence |
| --- | --- |
| play.png | [Whole run marked](../../../docs/quality/evidence/nonograms/05-whole-run.json) |
| large-board.png | [Zoomed window on a 25×25 board](../../../docs/quality/evidence/nonograms/08-large-board.json) |
| picture-completed.png | [Completed picture puzzle](../../../docs/quality/evidence/nonograms/16-picture-completed.json) |
| imported-puzzles.png | [Two pushed photos imported by name](../../../docs/quality/evidence/nonograms/18-imported-puzzles.json) |

Each JSON records source/binary digests, font provenance, scale, display profile and fixture identity. Reproduce with `scripts/quality/check-nonograms-sim.py` after building `kobo-cli` into the same cargo target directory.
