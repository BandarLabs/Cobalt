# CLI-06 - select among multiple readers without first-device fallback

Selection is explicit everywhere. Act commands take exactly one target; the
store can hold several readers and only the named one is used, and only when
its serial verifies. A name that matches no saved reader, or whose serial
answers nowhere, is a target error - never a fallback to the first reader
found. The interactive chooser (stream init without --device) lists what
answered and refuses to choose when several readers reply.

Evidence: transcript.txt - real runs: stream init usage showing --reader
(exit 2 on a bogus flag), saved-name resolution refusing to substitute
another reader. Tests: readers::tests::resolution_accepts_only_the_right_serial_at_a_saved_address,
an_unknown_name_is_a_target_error_that_names_the_known_readers;
targets::tests::one_target_only (exactly-one enforcement).
