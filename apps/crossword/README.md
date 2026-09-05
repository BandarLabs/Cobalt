# Crossword

This touch-first MVP includes a bundled grid, direction switching on a second tap, clue rotation, per-cell entry, and defensive `.puz` header validation. A letter redraws the grid only after that one input. The starter grid is sized for the Clara BW; 21×21 Sunday puzzles are intentionally gated until a larger-panel or scrolling layout is implemented.

`.puz` is an open file format. `.ipuz` import, the port 9337 transfer channel, autosave, rebus entry, and Crosshare fetching are deferred. Crosshare is AGPL-3.0 service software; commercial and NYT puzzles are never fetched.
