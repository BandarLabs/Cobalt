# Logic Pack

Four deterministic, touch-first pencil puzzles: Slitherlink, Hashi, Kakuro, and Minesweeper.
Each has a complete compact board, a reachable win state, short rules, and autosaved progress.
**Check** judges the current marks without revealing the answer. Minesweeper includes real adjacency
counts, flood reveal, flags, loss and completion states, and safely reseats a mine on the first tap.

![A driven Minesweeper state on Clara BW](screenshots/logicpack-mines.png)

The compact Slitherlink board has a unique loop for its four clues. Hashi uses a connected five-island
cross, and Kakuro uses a small cross-sum with one given. These intentionally small boards keep every
mark comfortably touchable on the smallest supported panel.

## Attribution and capabilities

The interaction architecture is informed by Simon Tatham's Portable Puzzle Collection (Loopy,
Bridges, Mines), MIT licensed; this app contains no copied implementation. Logic Pack has no
capabilities and never phones home.
