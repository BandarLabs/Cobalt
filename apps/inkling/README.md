# Inkling

A five-letter word puzzle, new each day, that works offline.

<table>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/solved.png" alt="A solved puzzle"><br>A solved puzzle</td>
<td width="50%" valign="top"><img width="300" src="screenshots/knowledge.png" alt="The board above the keys, with known letters"><br>The board above the keys, with known letters</td>
</tr>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/stats.png" alt="Statistics and win distribution"><br>Statistics and win distribution</td>
<td width="50%" valign="top"><img width="300" src="screenshots/archive.png" alt="The archive of past days"><br>The archive of past days</td>
</tr>
<tr>
<td width="50%" valign="top"><img width="300" src="screenshots/help.png" alt="Rules"><br>Rules</td>
</tr>
</table>

## Features

- Six guesses, with correct scoring for repeated letters and an optional hard
  mode.
- The board stays in view above the keys while you type. A letter in the
  right place is a filled square, a letter elsewhere in the word is in
  brackets, `(L)`, and a plain letter is not in the word. A word that is not
  accepted stays typed, so one wrong letter is one backspace.
- Above the board, the letters your earlier guesses have already proven are
  listed.
- **Rules** sits in the top bar, and the menu beside it holds New game (an
  earlier day), hard mode and statistics, where every Cobalt game keeps them.
- Saved daily progress, played and won totals, and a win distribution by
  number of guesses.
- **Export results** saves the day's board and statistics as
  `export-result.txt`, ready to fetch from a computer.
- The archive plays any of the past 365 days. Archive games do not count
  towards statistics and are not saved.

## How the daily word is chosen

The answer is picked from a hash of the date on the reader's own calendar,
so every reader gets the same puzzle each day without a server, and the next
one arrives at local midnight. The title shows the real date. For
simulator recordings, `KOBO_INKLING_DAY=YYYY-MM-DD` pins the day.

The word lists are hand-checked common English: 870 possible answers and
about 1,900 further accepted guesses. No commercial game's list is used.

## Permissions

None. Inkling runs offline.

## Development

```sh
cargo test -p kobo-inkling
python3 scripts/check-apps-sim.py inkling
```

The simulator check builds the app, opens it in a fresh simulator and plays
`drive.kobo`.
