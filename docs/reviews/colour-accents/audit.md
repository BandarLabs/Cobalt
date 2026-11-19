# Colour on every screen: the audit

Every application in the catalog and every built-in screen, reviewed from the
simulator sweep on `clara-colour-393` for where colour carries meaning and
where it would only decorate. The rule throughout is the one the palette is
built on: colour marks meaning the grey design already carries, on bands and
fills a few millimetres across, never on text, and a greyscale reader is
unchanged.

What every application gets without asking, because the runtime draws it:

- banners in the platform's green, red and amber, with their marks kept;
- the primary control, selected chips and tabs, switches that are on, ticked
  boxes, chosen options, progress fills, step marks and the selected cell of a
  pencil puzzle in the accent's strong shade (cobalt when the application has
  no accent);
- the square in hand on a chess or draughts board and a crossword's current
  word in the accent's tint; the dark squares of such a board and the shaded
  points of a backgammon table in brown;
- a refused text field tinted red under its note;
- in the status band, a battery at twenty percent or less in red, a charging
  one in green, a radio that is on with nothing getting through in amber.

What an application decides for itself: an accent, where it has a colour the
reader already knows, and pictures in colour, where it shows pictures worth it.

## Built-in screens

| Screen | Surfaces | Colour | Decision |
|---|---|---|---|
| Launcher | tile grid, nav bar, page rail | Cobalt bar, cobalt nav mark | Cobalt accent: this is the platform. |
| Store | rows, facts, tabs, banners, progress, QR | Cobalt bar and controls; red error banner | Cobalt accent. The QR code stays ink; a code is ink or it is not a code. |
| Settings | toggles, options, fields, steps, progress | Cobalt bar, cobalt switches and radio dots | Cobalt accent. |
| Books | tile grid of covers, banners | Cobalt bar | Cobalt accent. Covers are painted by Books itself in grey (`painted_cover`, `document_preview`); colouring generated covers is decoration and is left. |
| Terminal | terminal grid, keyboard | cobalt controls only | No accent. A terminal is ink on paper. |
| Boot splash, consent, restart notices | splash | cobalt field, cobalt primary | Platform cobalt. |

## Store applications

| App | Surfaces | Colour it gets | Decision |
|---|---|---|---|
| Preprints (arxiv) | rows, fields, banners, code, reader | Red bar and controls; figures in colour through the book view | Red accent: arXiv's own. |
| Backgammon | backgammon board, pictures (dice band), progress, facts | brown points, cobalt controls | No accent; the board carries the colour. Dice stay ink. |
| Birds | collage picture, facts, banner | Green bar; collage already in colour | Green accent: the one subject here that is the outdoors. |
| calibre-web | steps, banners, fields, keyboard | cobalt controls, green/red banners | No accent. No brand colour worth borrowing; no covers are shown. |
| Crossword | crossword grid, keyboard, progress | current word in cobalt tint; blocks ink | No accent. The word being solved is the one thing worth colour. |
| Deck | grid of pads, tabs, banners | cobalt controls, red and amber banners | No accent. |
| Fanshelf | rows, table, quote, fields, banners | cobalt controls | No accent. Quiet by design. |
| Fieldbook | rows, checked rows, table, photo, fields, banners | photos in colour; cobalt checks; green banner | No accent. A notebook is paper. |
| Flashcards | progress, table, picture, rich text | card images in colour; cobalt progress and buttons | No accent. |
| Frame | full-bleed photo, settings | photos in colour | No accent. The photo is the whole point and gets all the colour. `kobo frame push` prepares photos on the computer, so it now keeps their colour when the reader's profile has a colour panel, as a JPEG under the entry's usual name; a greyscale reader's photos are reduced to grey there as before. |
| Grimoire | tile grid, tabs, facts, options | cobalt controls | No accent. |
| Habits | tabs, table, options, banners | cobalt controls | No accent. |
| Home Panel | grids, tiles, toggles, steps, fields | cobalt switches and steps | No accent. |
| Inkling | board, keyboard, toggles, banners | cobalt controls | No accent. |
| Kitchen Card | tabs, steps, facts, checked rows, banners | cobalt controls, green/red banners | No accent. |
| Lichess | chess board, grids, tiles, chips, terminal, banners | brown dark squares, cobalt tint on the square in hand, cobalt chips | No accent. The board is the colour; a brown bar over it would be livery. |
| Logic Pack | pencil boards, boards, progress, fields | selected cell in cobalt, red tint on a refused field | No accent. Minesweeper marks every covered square as selected; those stay grey, because fifteen tinted squares are a field and not a mark. |
| Morse | board, letter picture, keyboard, toggle | cobalt controls | No accent. |
| Music Stand | sheet music pictures, toggles, banners | cobalt controls | No accent. Sheet music is black on white by nature; pictures stay grey. |
| Needles | progress, tabs, facts, options, banners | cobalt controls | No accent. |
| Nonograms | board marks, picture, progress, tabs, toggles | cobalt progress and switches | No accent. Marks stay ink: the puzzle is the ink. |
| Panels | comic pages, progress, toggles | pages already in colour | No accent. |
| PaperTerm | terminal, grids, keyboard | cobalt controls | No accent. |
| Parlor | boards, grids, tables, terminal | brown squares where a board has them | No accent. Reversi's marked disc keeps its grey square: a plain grid's selection means whatever the game says. |
| Parser | board, grids, tables, keyboard | cobalt controls | No accent. |
| Post | board, keyboard, fields, banners | red tint on a refused field | No accent. |
| Pub Quiz | grid, tables, fields, banners | cobalt controls | No accent. |
| Readeck | steps, banners, fields, reader | article pictures in colour through the book view | No accent: no brand colour to borrow. |
| Read Later | tabs, code, banners, reader | article pictures in colour | No accent. |
| Miniflux (rss-miniflux) | tabs, banners, fields, reader | article pictures in colour | No accent. Miniflux's own mark is not orange, and its first screens are banners; an orange bar over the amber banners of its setup screens was too much. |
| Sudoku | pencil board, progress, tables | selected cell in cobalt | No accent. Conflicts are not marked by the app today; a red cell would need the app to detect them first, and is noted below. |
| Syncthing | facts, toggle, options, banners | Blue bar, blue switch | Blue accent: Syncthing's own, kept apart from cobalt. |
| Vault | grid, code, banners | cobalt controls | No accent. |
| Verses | chips, rich text, banners | cobalt chips | No accent. |
| Stacks (zotero-reader) | picture, steps, banners, fields, reader | Red bar and controls; figures in colour | Red accent: Zotero's own. |

## Example applications shipped from the repository

| App | Colour it gets | Decision |
|---|---|---|
| Hacker News | Orange bar and controls; article pictures in colour | Orange accent. |
| Feeds (rss) | Orange bar and controls; article pictures in colour | Orange accent. |
| Audiobooks | cobalt controls | No accent. Cover art is generated, in grey. |
| Daily Brief | cobalt radio dots | No accent. |
| AI Chat | cobalt radio dots, amber banner | No accent. |
| Gallery | every control, in cobalt | No accent; it is the catalogue of controls. |
| Gutenbird | book covers in colour; cobalt controls | No accent. |
| Magnet | cobalt controls | No accent; its diagram is ink. |
| Sidekick | cobalt controls | No accent. |
| Tic-tac-toe, Todo, Hello | cobalt controls | No accent. |

## Left out on purpose, and why

- **A red cell for a wrong digit in Sudoku and Logic Pack.** The clearest
  colour these games could carry, and the one most asked for, but neither
  application detects a conflict today; the pencil board would also need a
  state on the wire for it. Worth doing as its own change once the apps mark
  conflicts.
- **A destructive button in red.** The wire has one emphasis, primary, and
  Fieldbook's Delete is drawn in cobalt like any primary control. A red
  destructive variant is a protocol addition and a design decision in its own
  right.
- **Coloured link underlines in reading text.** Reading surfaces stay quiet;
  colour at half the grey resolution under a word is a soft smudge.
- **Generated covers and diagrams in colour** (Books, Audiobooks, Magnet,
  Backgammon's dice). These are drawn by the apps in ink and read well as ink.
- **Accents for apps with no colour of their own.** Most utilities are better
  without one; a Store where every bar is a different colour is a set of
  themes, not a platform.
