# Logic Pack

Four offline games: Slitherlink, Hashi, Kakuro and Minesweeper. Each game keeps
its own progress when you return to the puzzle list or reopen the app.

![Minesweeper on Clara BW](screenshots/logicpack-mines.png)

**Check** judges your marks without revealing the answer. **Undo** reverses up
to 32 changes per game, including checks, flags, flood reveals and the mine
relocation that makes the first reveal safe. Undo history survives reopening.
**Restart** asks before clearing this game, preserves the other games, and can
itself be undone. Help is in the top bar; Back returns to the puzzle list.
Finished fields offer Undo, Restart and Puzzles instead of inactive input modes.

The app starts in portrait. Controls use two columns and retain readable labels
at every supported text size. Layout checks use the shipped fonts on Clara BW
and the smaller 758 × 1024 display at all nine text sizes.

Progress shows **Saving…** until storage confirms it. An older write cannot
mark newer moves saved. A failed write keeps your latest moves in memory and
offers **Retry save**; keep the app open until saving succeeds. Unreadable or
future-version records are left untouched and offer a read retry. Existing
single-game saves migrate on the next edit. The bounded record holds all four
games and their undo history.

The current edition still contains four compact starter boards: a 2 × 2
Slitherlink, a five-island Hashi, a small Kakuro with one given, and 4 × 4
Minesweeper. A larger validated collection with difficulty guides and proper
line, bridge and cross-sum rendering remains on the quality checklist. These
screenshots document the current implementation, not final acceptance of that
board design.

`python3 scripts/quality/check-logicpack-sim.py --output /tmp/logicpack-check`
from the repository root drives the actual SDK simulator. It checks all four
completions and forced reopenings, independent progress, persistent undo,
failed saves and retry, restart confirmation, legacy migration and unreadable
record preservation. It uses a private temporary store and asserts no network
effects. Physical Clara BW acceptance follows the three quality PRs.

Logic Pack requests no capabilities and uses no external puzzle code or assets.
