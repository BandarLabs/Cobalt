# Habits

A fully offline habit tracker. Habits and completions stay on the reader;
nothing connects an account or uploads progress. Settings can export a backup
as a verified text copy for your paired computer, and import one back from the
shelf. Replace removes the habits on the reader. Merge keeps them and adds
days from the backup when the name matches. The original stays on the reader
until you confirm.

| The empty Today offer | A finished day |
| --- | --- |
| ![Nothing due, with the Add a habit button offered](screenshots/empty.png) | ![The read habit checked off and labelled Done](screenshots/done.png) |

| Editing a habit | The week on Stats |
| --- | --- |
| ![Rename, the three schedules with the current one ticked, and Archive](screenshots/edit.png) | ![4 completions; this week: 4 of 11 due days completed, 1 skipped](screenshots/stats.png) |

| The backup, ready |
| --- |
| ![habits-backup, a 1 KB text copy, ready for a paired computer](screenshots/export.png) |

*Captured by `scripts/quality/check-habits-sim.py` on the simulator's Clara BW
profile, end to end with no network at all.*

## Repaint policy

Tapping a habit repaints that row once for its checked state.
