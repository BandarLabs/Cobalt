# Crossword simulator acceptance

Run `python3 scripts/quality/check-crossword-sim.py --output /tmp/crossword-check`
with the current `kobo` binary in `CARGO_TARGET_DIR/debug`. This result uses Clara
BW 391, extra-large text, the actual SDK app and private temporary storage.

The 17 checks cover word entry, numbered blocks, both clue directions, check,
reveal, completion, restart, undo, failed-write preservation and explicit retry.
The committed drive route also runs. Finally, a legacy record is opened without
rewriting it, migrated on an edit, reopened, and replaced within the private
fixture by unreadable bytes to verify that Retry preserves them.

Every PNG has source, binary, font and profile metadata; layout files accompany
the main journey. Every capture asserts zero fetch/post effects. The source was
an accurately reported dirty working tree before the implementation commit.
No physical reader or personal storage was used. Crossword deliberately uses
portrait to keep its clue and keyboard together. Unit layout checks cover all
nine text sizes on the 1072×1448/300 ppi and 758×1024/212 ppi portrait profiles.

The pack has one original blocked mini with ten distinct answers and three word
squares, including the unchanged original 5×5 answer. This edition does not
advertise `.puz`, `.ipuz`, rebus or Sunday-grid import support.
