# Sudoku screenshots

The SDK app in the Clara BW simulator at 1072 × 1448, taken from fresh simulator storage. These are ideal grayscale simulator frames, not physical-reader captures. All puzzles are original and bundled with the app.

| Image | What it shows |
| --- | --- |
| game.png | Easy 1 with three pencil notes in the first square and an answer in the next |
| completed.png | Easy 1 filled in, with the status naming New game |
| landscape.png | Hard 1 with pencil notes, after More → Use landscape |
| recovery.png | More open after a save failed, with Retry saving under New game |

`game.png` is the `sudoku-entries` shot in the app's `drive.kobo` route, at the default text size. The other three were taken earlier; `landscape.png` and `recovery.png` still show the bar's old **Digits** label. To take those again, start `kobo dev` from `apps/sudoku` with `KOBO_TEXT_SCALE=extra-large` and an empty `TMPDIR`, then drive: finish Easy 1; choose More → New game → Hard; switch on Notes and add each square's possible numbers in the first eight empty squares of the top band; select the third. Then More → Use landscape for `landscape`, back to portrait, `scenario storage-full`, add one note and open More for `recovery`.

The public app page uses `game.png` at `docs/media/site/apps/sudoku.png`.
