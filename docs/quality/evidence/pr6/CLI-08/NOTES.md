# CLI-08 - preserve verified USB setup, eject, reboot and reconnect steps

The USB flow keeps its verified order: identify the mounted reader from the
firmware's own .kobo/version, build or verify the whole payload before
anything is written, write, verify what landed, apply settings, carry trust
roots, eject, and only then wait for the reader that was restarted by the
eject to reappear on the network (only when SSH was enabled; a reader never
ejected has not seen the install, so there is nothing to wait for).
`reader_still_connected` re-checks the same serial between steps.

Evidence: transcript.txt - a real dry run against a mounted volume carrying
a real-format version line: the reader is identified (Kobo Clara BW, device
code 391, firmware 4.45.23697) and the plan names the eject and the
wait/skip decision. Tests: tests::preparing::a_bare_run_installs_ejects_and_waits,
a_dry_run_of_an_undo_describes_the_undo_and_performs_nothing, plus the
setup.rs payload/rollback/verify suite. The live install path itself cannot
run in this sandbox (no ARM cross-compiler); that is an environment limit,
not a gap in the steps.
