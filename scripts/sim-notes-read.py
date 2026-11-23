#!/usr/bin/env python3
"""Mark the launcher's What's new notes as read in a simulator's fresh state.

The launcher shows its What's new sheet once, over the grid, to a reader that
has not seen the notes for the running version. A fresh simulator is such a
reader on every start, so from the day the workspace version matches
examples/launcher/whats-new.txt every harness that starts a launcher would
open on the sheet, with its first tap landing on nothing.

A harness stands for a reader whose owner has already read the notes. This
writes what the launcher itself writes when Done is tapped: the version the
notes are for, under the launcher's `whats-new-seen` key. It is the one place
that knows the key and where the file lives, so a harness calls it where it
prepares its state and knows neither.

usage: scripts/sim-notes-read.py TMPDIR

TMPDIR is the temporary folder the simulator will be started with. The state
goes where the simulator keeps it: in `KOBO_SIM_DATA/.state` when that is set
for this command as it will be for the simulator, and otherwise in
`TMPDIR/cobalt-sim-state`.
"""
import os
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parent.parent
NOTES = ROOT / 'examples/launcher/whats-new.txt'
# The launcher's store key: `SEEN` in examples/launcher/src/whats_new.rs,
# whose tests check that this file still names it.
SEEN = 'whats-new-seen'


def notes_version():
    """The version the notes are for: their first line that is not a comment."""
    for line in NOTES.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith('#'):
            return line
    raise SystemExit(f'{NOTES} names no version')


def state_folder(temporary):
    chosen = os.environ.get('KOBO_SIM_DATA')
    return Path(chosen) / '.state' if chosen else Path(temporary) / 'cobalt-sim-state'


def main():
    if len(sys.argv) != 2:
        raise SystemExit(__doc__.strip())
    folder = state_folder(sys.argv[1]) / 'launcher'
    folder.mkdir(parents=True, exist_ok=True)
    (folder / SEEN).write_text(notes_version())


if __name__ == '__main__':
    main()
