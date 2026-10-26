# CLI-17 - persist receipts and resume approved interrupted transfers

Every acknowledged send appends a receipt to ~/.config/kobo/receipts
(KOBO_CONFIG_DIR overrides): file, companion, target, content hash, epoch
time. A receipt exists only past the companion's acknowledgement, so an
interrupted transfer leaves none - which is what makes the next send of
that file a resume rather than a duplicate. Re-sending unchanged bytes to
the same target is reported as already done (exit 0, nothing pushed);
changed bytes hash differently and send fresh, and the ledger records them.
Frame's shelf is itself content-addressed, so a re-sent image that
re-encodes to the same bytes is kept once on the reader side too.

Evidence: transcript.txt - real sends: first send records a receipt, the
repeat reports "unchanged since it was sent", an edited file sends fresh
and the ledger grows. (One run in the transcript was deliberately piped
through head and died to SIGPIPE before its receipt write - visible as a
send with no ledger entry, which is the interruption behavior itself.)
Tests: receipts::tests::* (round-trip, changed-file/different-target miss,
interrupted-send absence).
