# CLI-19 - retain selection and preparation on retry

A send's whole selection - file, settled companion, target flags exactly as
given - is written to ~/.config/kobo/pending-send before the dispatch runs,
so even a crash keeps it. A failed send's error names the way back:
`kobo send --retry` rebuilds the identical command (with a "retrying"
progress line), and a successful send clears the pending file. --retry with
nothing waiting is a target error, exit 3.

Evidence: transcript.txt - real runs: unreachable reader keeps the
selection (pending-send shown), --retry re-runs it and fails honestly
again, a successful send clears the pending file, and a bare --retry
reports nothing waiting. Tests: receipts::pending_tests::*.
