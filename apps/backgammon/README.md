# Backgammon

This portrait MVP uses a 24-point, touch-first board chosen to keep every point visible on a 1072×1448 Clara BW. Tap **Roll**, a checker, then its marked destination. The deterministic 36-roll cycle is uniform and is checked by `deterministic_dice_sequence_passes_uniformity_check`; production entropy should replace it before release.

The present engine covers opening positions, blocked points, hits, checker movement, turn changes, and a simple cube offer. Bar re-entry, bearing off, Crawford matches, beavers, pass-and-play rotation, and the planned heuristic AI are deferred. GNU Backgammon is deliberately not used; this is not a bot-market GM.
