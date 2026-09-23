# CLI-18 - publish atomically and retain prior valid content

One shared publish::atomically now owns the transfer endgame: bytes go to a
.writing sibling, sync to storage, then rename over the destination - one
directory operation, so the shelf always holds the whole old file or the
whole new one. A leftover partial is the visible remains of an interrupted
write: it blocks the next publish with "existing content unchanged" until
the owner removes it. Feeds publishes through it (its retention test
intact); Frame's verified push and Panels' hash-checked device move keep
their own paths, named here rather than churned.

Evidence: transcript.txt - real sends against the simulator shelf: a stale
.writing partial blocks the publish (exit 1, selection kept for --retry),
removing it lets the two-feed list land, and a corrupt list is refused with
the previous valid list untouched. Tests: publish::tests::*,
feeds::tests::publication_preserves_existing_list_when_staging_is_unavailable.
