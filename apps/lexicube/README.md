# Lexicube

Sixteen letter dice, three minutes, and a dictionary for the argument
afterwards.

The board is the game: players read the same shaken board and write every word
they can find on their own paper, exactly as they would around a physical
tray. The app supplies the shake, the clock, and the part every table wants
when the pens go down — settling whether a word is real.

## Playing

Shake with **New game**. The dice land rotated, as plastic dice do, drawn in a
bold serif whose serifs keep a turned N from reading as a Z. A word uses
letters that touch in a chain, sideways or diagonally, without using the same
die twice; three letters or longer, and the Qu die counts as two letters.

**Pause** covers the board with a letterless twin of the same size, so
stopping the clock never hands anybody extra thinking time. **End the game
early** sits under the clock for tables that run out of words before the
clock does.

## When the pens go down

**Check a word** settles disputes: validity is answered offline from an
embedded SOWPODS list of 267,750 words, misspellings get near-miss
suggestions, and definitions come from Cobalt's offline dictionary service
when the owner has installed dictionaries. Scoring is a word's letter count
minus two — one point for a three-letter word, and one more for every letter
beyond it. A plural is its own word, found and scored beside its singular.

## Licences

The word list is the SOWPODS compilation as packaged by `pf-sowpods` (MIT).
The dice letters are set in DejaVu Serif Bold, whose Bitstream Vera licence
travels beside it in `fonts/LICENSE-DejaVu.txt`. The dice face distribution
is the modern tabletop set's.

---

Built with the [Cobalt SDK](../../README.md). No capabilities: the panel and
the embedded list are the whole game.
