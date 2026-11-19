# CLI-13 - choose files/folders through picker or explicit CLI path

Both halves exist and share one code path. The explicit half: `kobo send
FILE ...` takes the path on the command line, checks it is a real file on
this computer (a missing one is a usage error, exit 2), and never expands or
rewrites it. The picker half: guided choice 7 asks for the path in plain
text - a terminal's picker is a typed path, not a browse dialog - then asks
where it goes with the shared numbered picker (simulator, reader by address,
saved reader by name); blank answers cancel at every step. The menu builds
the same `kobo send` argv and dispatches through the same registry.

Evidence: transcript.txt - a real PTY run: choice 7, typed path, numbered
destination pick, and the same send completing on the simulator shelf.
Tests: owner_start::tests::every_choice_dispatches_to_a_real_command covers
the new choice; the picker paths cancel cleanly per the existing menu tests.
