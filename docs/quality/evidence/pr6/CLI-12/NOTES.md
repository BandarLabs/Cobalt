# CLI-12 - persist completed setup steps

Completed steps live in ~/.config/kobo/setup-steps (KOBO_CONFIG_DIR
overrides), the same key=value block format as the reader store: step name,
reader serial, epoch seconds. A step is recorded only after the work it
names finished - the USB setup records `setup` past the eject, so a dry
run, a declined confirmation or a failed verify completes nothing.
Re-recording a step for the same reader refreshes it rather than stacking a
duplicate. The guided menu reads the store back and says where the owner
is. A store that cannot be written never fails the work it would have
recorded (the install prints a note and carries on).

Evidence: transcript.txt - real PTY run with a recorded setup: the menu
greets with "Done on this computer: reader setup (serial N365…)"; with an
empty config the line is absent. Tests: steps::tests::steps_round_trip,
completing_twice_refreshes_instead_of_stacking, a_missing_file_is_no_steps.
The live-install write path is gated only by the sandbox's missing ARM
cross-compiler; the store and its read-back are exercised for real here.
