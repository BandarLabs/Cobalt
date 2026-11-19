# CLI-09 - send and open an original sample for first success

A successful USB setup now writes an original short welcome note,
"Welcome from Cobalt.txt", into the reader's library (setup::write_sample),
synced before the eject, so the first reconnect shows something new to open.
It is owner-language, says what setup did, is deletable, and never fails the
install. `--no-sample` skips it; the dry-run plan names it either way. The
final report tells the owner to open it on the reader after the restart.

Evidence: CLI-10's transcript shows the dry run naming the note, and
setup::tests::the_welcome_note_lands_beside_the_library_and_is_deletable
proves the write against a real mounted-volume path. Sending in a live
install is the same write_payload-adjacent filesystem write, gated here only
by the sandbox's missing ARM cross-compiler - stated plainly, not simulated.
