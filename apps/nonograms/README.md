# Nonograms

Sixty original bundled picture grids, from 5×5 through 25×25, are checked at
build time by the same row-and-column solver the app uses for guided play.
Each has a no-guess solution: if line deductions cannot finish a generated
candidate, it is not shipped. The browser records solved puzzles without
showing their pictures first.

Tap a square to cycle fill, X, blank. Turn on **Run entry** to tap both ends
of a horizontal or vertical filled run when a drag is not available. Guided
mode names the first row or column made impossible; free mode keeps mistakes
private until completion. Every move is autosaved under its puzzle id.

The solve replaces the grid in one large GL16 transition: bundled art is
rendered as a 16-grey source drawing, and a photo puzzle uses its processed
photograph. The platform's refresh planner schedules its normal GC16 cleaning
refresh from the cumulative dirty-pixel budget.
The Clara BW's current 81-cell grid limit means 15×15 and 25×25 pack entries
are browseable but gated for play; 5×5, 7×7, and 9×9 are touch-tested there.

## Photo puzzles

`kobo nonograms push photo.jpg --size 9 --device READER` decodes an owner
image on the host, applies its EXIF orientation, centre-crops and reduces it
to a deterministic greyscale thresholded PNG, then atomically transfers the
bounded `photo.png` to the reader. Use `--out photo.png` instead of
`--device` to inspect the exact file first. Choose the same grid size in the
app before opening it; the Clara BW command accepts 5, 7, or 9.
Photo import requires Cobalt 0.3.2 or newer, which reads transferred files
from each app's private data directory.

The app samples the transferred PNG and accepts it only when line solving
proves the result fair; otherwise it says, “This one does not make a fair
puzzle.” Its photo puzzle id derives from the transferred content and grid
size, so replacing a photo cannot inherit old progress or solved state.

## Attribution

The line solver is independently implemented. Tatham's Puzzle Collection
**Pattern** solver is an MIT-licensed algorithm reference; it is not linked
or used at runtime. `Picross` is Nintendo's trademark and is not used by
this app.

## Validation

```sh
CARGO_TARGET_DIR='/Volumes/Untitled 1/cobalt-targets/batch-08' cargo test -p kobo-nonograms
CARGO_TARGET_DIR='/Volumes/Untitled 1/cobalt-targets/batch-08' cargo clippy -p kobo-nonograms -- -D warnings
```
