# Nonograms screenshots

Actual SDK output in the Clara BW simulator. These are ideal grayscale frames, not hardware captures.

| Image | What it shows |
| --- | --- |
| play.png | A whole run marked on a 5×5 picture |
| large-board.png | A zoomed window on a 25×25 board |
| picture-completed.png | A completed picture puzzle |
| imported-puzzles.png | Three pushed photos imported by name |

`play.png` comes from the app's `drive.kobo` route. `scripts/quality/check-nonograms-sim.py` drives the other three screens and writes each capture with its source digests, font provenance, scale and display profile. Build `kobo-cli` into the same cargo target directory first.
