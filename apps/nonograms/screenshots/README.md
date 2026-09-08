# Nonograms screenshots

Actual SDK output in the Clara BW simulator at extra-large interface size. These are ideal grayscale frames, not hardware captures.

| Image | Evidence |
| --- | --- |
| nonograms-play.png | [Selected square and matching clues](../../../docs/quality/evidence/nonograms/02-marked-square.json) |
| large-grid.png | [Last square of a 25×25 board](../../../docs/quality/evidence/nonograms/09-last-square.json) |
| recovery.png | [Save retry](../../../docs/quality/evidence/nonograms/06-save-recovery.json) |
| clue.png | [Complete row clue](../../../docs/quality/evidence/nonograms/03-full-row-clue.json) |

Each JSON records source/binary digests, font provenance, scale, display profile and fixture identity. The public app page uses the selected-square capture. Reproduce with `scripts/quality/check-nonograms-sim.py` after building `kobo-cli` into the same cargo target directory.
