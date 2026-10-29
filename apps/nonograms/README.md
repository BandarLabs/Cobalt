# Nonograms

Eighteen original picture puzzles, from 5×5 to 25×25, plus puzzles made from
your own photos.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/play.png" alt="Marking a run on the board"><br>Marking a run on the board</td>
<td width="50%" valign="top"><img width="300" src="screenshots/large-board.png" alt="A 25×25 board, zoomed"><br>A 25×25 board, zoomed</td>
</tr>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/picture-completed.png" alt="A finished picture"><br>A finished picture</td>
<td width="50%" valign="top"><img width="300" src="screenshots/imported-puzzles.png" alt="Photo puzzles listed by name"><br>Photo puzzles listed by name</td>
</tr>
</table>

## Features

- Tap a square to cycle through blank, filled and crossed out. The puzzle is
  finished when every square is filled or crossed correctly.
- The selected square's row and column clues are highlighted. Tap a clue to
  read all of it. **…** means there are more numbers than fit.
- Large boards show a movable window instead of shrinking the squares. Use the
  arrow controls to move it and **−** and **+** to change the square size.
- **Run on** fills a straight run between two taps.
- **Guided** mode names a row or column whose marks contradict its clues. It
  never changes your marks. **Free** mode gives no warnings.
- **Undo** reverses one square, a run or a restart. The last 64 steps survive
  reopening. **Restart** asks first.
- Three tabs: **Pictures**, the 60 **Patterns** puzzles that shipped first
  (progress kept), and **Photos**, your own.

Each puzzle saves separately. If a save fails, your moves stay in memory and
**More → Retry save** tries again. Finish saving before switching puzzles or
closing the app.

## Photo puzzles

Turn a photo into a puzzle from your computer:

```sh
kobo nonograms preview IMAGE --out DIRECTORY
kobo nonograms push IMAGE --name NAME --size N --device IP
kobo nonograms push first.jpg --name Harbour --size 15 \
  --add second.jpg --name Garden --size 20 --device IP
```

Set up the reader once over USB with `kobo setup`, if you have not already.
`preview` shows the puzzle a photo would make before you send it. Sizes run
from 5 to 25. Each push adds to the photo puzzles already on the reader and
prints the list; the reader keeps up to 24. `--replace` keeps only the photos
you name. Sending an unchanged photo again keeps its progress. The puzzles
appear under **Photos** the next time Nonograms opens or the reader wakes.

A photo is accepted only if it makes a puzzle with exactly one solution. The
difficulty shown (Easy, Medium or Hard) reflects how many solving passes it
needs, not measured human solving time.

## The drawings

The eighteen pictures are original. `scripts/quality/make-nonogram-pictures.py`
builds them, confirms each has a single solution and writes
[assets/pictures.txt](assets/pictures.txt).

## Permissions

None. Nonograms runs offline.

## Development

```sh
cargo test -p kobo-nonograms
python3 scripts/check-apps-sim.py nonograms
```

The simulator check builds the app, opens it in a fresh simulator and plays
`drive.kobo`. `scripts/quality/check-nonograms-sim.py --output /tmp/nonograms-check --scale extra-large` runs a longer check, including the last square of a 25×25 board and failed saves. See [screenshots/README.md](screenshots/README.md) for how the screenshots were made.
