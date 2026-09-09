# Logic Pack screenshots

Actual SDK simulator captures on Clara BW at Extra-large:

- `logicpack-mines.png`: the committed route with a partially revealed field.
- `completed.png`: cleared field with flagged mines and contextual actions.
- `save-failed.png`: retained moves after an injected full-store failure.
- `restart.png`: confirmation before clearing one game.

Matching captures, layouts and source/font provenance are under
`docs/quality/evidence/logicpack/progress-recovery`. No network effects occur.
These earlier captures cover the original four puzzles; physical acceptance remains pending.

`hashi.png`, `kakuro.png` and `slitherlink.png` show the shared pencil-board
component: circular islands, attached diagonal sum clues and continuous loops.
`double-bridge.png` and `excluded-edge.png` show alternative edge marks. These
actual Extra-large captures come from `evidence/logicpack/pencil-boards`; the
17-check route retains all progress/recovery checks and verifies the new geometry.
The SDK guide uses the same Hashi capture. Earlier evidence retains its earlier
generic-cell rendering for comparison.

The collection captures (`collection.png`, `entry.png`, `collection-slither.png`)
come from `docs/quality/evidence/logicpack/collection`. The actual app completed
and forcibly reopened all 20 puzzles on Clara BW at Extra-large. The 16 check
groups include direct digit entry, both legacy migrations, full-store retry,
persistent undo and future-record preservation. Layouts and source/font
provenance accompany every capture. No network effects occur.
