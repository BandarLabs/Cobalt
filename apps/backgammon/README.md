# Backgammon

This is a portrait 24-point board for the 1072×1448 Clara BW. It draws conventional opposing triangular points, vector white and black checker stacks, a central bar, off-board trays, dice, and the doubling cube; pass-and-play reverses the board so the active player is nearest the reader. Tap **Roll**, a checker (or the active bar), then a hollow destination. Legal-turn generation enforces forced moves, maximum dice use, the higher-die rule, bar-entry priority, blocks, hits, doubles, exact and oversize bear-offs, and automatic no-move turns.

Matches may be 1, 3, 5, or 7 points. The doubling cube has an explicit offer/take/drop sequence, owner discipline, and Crawford-game restriction; beavers remain off. Games score singles, gammons, and backgammons. Every action is autosaved and a move may be undone before the turn ends. Solo mode includes a deliberately modest positional computer player; pass-and-play reverses the board for the next player.

The dice cycle visits every ordered pair equally and is checked by `deterministic_dice_has_equal_ordered_pair_counts`. Runtime entropy is pending a platform RNG API. GNU Backgammon is deliberately not used. The computer is a modest one-ply heuristic, not a bot-market GM.
