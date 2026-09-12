# Quality work reconciliation · 12 September 2026

Fetched `origin/beta` and `origin/beta-quality-apps-2` before comparing the checklist with GitHub. PR #167 is merged; PR #168 merged as `a409e7e8` and `beta-v0.3.14` was published on 12 September. PR #181 starts from that merged work. The earlier uncommitted SDK snapshot and Miniflux work is no longer a separate outstanding patch: the current catalog batch records Miniflux complete. Do not reapply the older workspace inventory.

The source of truth is `tasks.json`: 496 unique tasks, 205 done, 290 open and one deferred. PR 1 has 92 done and CBR deferred; PR 2 has 113 done; PR 3 has 157 open; PR 4 has 133 open. Checklist checkboxes were compared with every task status. No status changed during reconciliation. OWNERQA-18 and OWNERQA-21 now name the revised four-PR plan instead of the superseded three-PR limit.

The shared tile-label change already in the worktree belongs to the ongoing Gutenberg investigation. It was preserved separately from this reconciliation and the Lichess recovery changes. A dirty patch is not evidence of completed validation.

## Remaining catalog scope · PR #181

| Group | Open tasks |
| --- | ---: |
| arXiv | 5 |
| Audiobook Studio | 6 |
| AI Command Center | 5 |
| Fanshelf | 6 |
| Fieldbook (APP-3) | 8 |
| Flashcards app | 6 |
| Frame app | 6 |
| Gutenbird | 6 |
| Habits | 6 |
| Home Panel | 7 |
| Inkling | 7 |
| Kitchen Card (APP-2) | 8 |
| Lichess | 6 |
| Morse | 6 |
| Music Stand (APP-4) | 7 |
| Needles | 6 |
| Parser | 6 |
| Post | 6 |
| Pub Quiz | 6 |
| Read Later (APP-5/6) | 6 |
| Sync app | 6 |
| Vault (APP-6) | 7 |
| Verses (APP-8) | 6 |
| Catalog-wide release gates | 13 |

## Remaining companion scope · PR 4

No companion PR is open at this checkpoint. Its 133 tasks remain planned; the branch name is `beta-quality-companion`.

| Group | Open tasks |
| --- | ---: |
| Main CLI and companion operation engine | 38 |
| Deck companion | 7 |
| Flashcards companion | 9 |
| Frame companion | 7 |
| Vault companion | 7 |
| Sync companion | 6 |
| Needles companion | 6 |
| Nonograms companion | 4 |
| Parser companion | 4 |
| Paperterm companion | 5 |
| Sidekick companion | 6 |
| Provider connections | 8 |
| Missing companion workflows | 5 |
| Owner experience and final validation | 21 |

## Priorities and acceptance

Continue the agreed reading-app order: Gutenbird, arXiv, Verses, Read Later, Frame and Sync, followed by Lichess and Music Stand. Treat the owner-reported Lichess matched-but-waiting defect as an immediate recovery issue. Preserve the newspaper crossword design and portrait Paperterm behavior already delivered. Live authenticated Lichess play, physical launch latency and other Clara BW acceptance remain distinct from simulator evidence. Do not mark those passed from a host render or an API fixture.
