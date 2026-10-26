# CLI-36: preserve content, preferences and pairing through updates

Two mechanisms, both evidenced:

1. The owner's stores are forward-compatible by contract: readers, receipts,
   pending-send and steps parsers ignore unknown lines, so a store written
   by a newer kobo keeps working on an older one (and vice versa). The
   transcript performs the surgery for real - a `future_field` line appended
   to the readers store, `kobo report` still reads both pairings; a
   `future_flag` line appended to receipts, `kobo send` still reads the
   ledger (the dedup line proves it).
2. `kobo update` replaces only the managed host binary (install-state under
   the data directory); configuration, saved pairings, receipts and steps
   live under the config directory and reader content on the reader, none
   of which the update path touches. Source checkouts are refused with
   "use git and cargo", so an update can never clobber a development
   checkout either.

A full managed-update run needs an installed build and release hosting,
which this sandbox does not have; that half is evidenced from the update
code path (main.rs update_host) and labeled here rather than staged.
